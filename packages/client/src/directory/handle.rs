use super::{Builder, Data, Id, Object};
use crate as tg;
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tangram_either::Either;

#[derive(Clone, Debug)]
pub struct Directory {
	state: Arc<std::sync::RwLock<State>>,
}

pub type State = tg::object::State<Id, Object>;

impl Directory {
	#[must_use]
	pub fn with_state(state: State) -> Self {
		let state = Arc::new(std::sync::RwLock::new(state));
		Self { state }
	}

	#[must_use]
	pub fn state(&self) -> &Arc<std::sync::RwLock<State>> {
		&self.state
	}

	#[must_use]
	pub fn with_id(id: Id) -> Self {
		let state = State::with_id(id);
		let state = Arc::new(std::sync::RwLock::new(state));
		Self { state }
	}

	#[must_use]
	pub fn with_object(object: impl Into<Arc<Object>>) -> Self {
		let state = State::with_object(object);
		let state = Arc::new(std::sync::RwLock::new(state));
		Self { state }
	}

	#[must_use]
	pub fn id(&self) -> Id {
		if let Some(id) = self.state.read().unwrap().id.clone() {
			return id;
		}
		let object = self.state.read().unwrap().object.clone().unwrap();
		let data = object.to_data();
		let bytes = data.serialize().unwrap();
		let id = Id::new(&bytes);
		self.state.write().unwrap().id.replace(id.clone());
		id
	}

	pub async fn object<H>(&self, handle: &H) -> tg::Result<Arc<Object>>
	where
		H: tg::Handle,
	{
		self.load(handle).await
	}

	pub async fn load<H>(&self, handle: &H) -> tg::Result<Arc<Object>>
	where
		H: tg::Handle,
	{
		self.try_load(handle).await?.ok_or_else(|| {
			tg::error!(
				"failed to load the object {}",
				self.state.read().unwrap().id.as_ref().unwrap()
			)
		})
	}

	pub async fn try_load<H>(&self, handle: &H) -> tg::Result<Option<Arc<Object>>>
	where
		H: tg::Handle,
	{
		if let Some(object) = self.state.read().unwrap().object.clone() {
			return Ok(Some(object));
		}
		let id = self.state.read().unwrap().id.clone().unwrap();
		let Some(output) = Box::pin(handle.try_get_object(&id.into())).await? else {
			return Ok(None);
		};
		let data = Data::deserialize(output.bytes)
			.map_err(|source| tg::error!(!source, "failed to deserialize the data"))?;
		let object = Object::try_from(data)?;
		let object = Arc::new(object);
		self.state.write().unwrap().object.replace(object.clone());
		Ok(Some(object))
	}

	pub fn unload(&self) {
		self.state.write().unwrap().object.take();
	}

	pub async fn store<H>(&self, handle: &H) -> tg::Result<Id>
	where
		H: tg::Handle,
	{
		tg::Value::from(self.clone()).store(handle).await?;
		Ok(self.id())
	}

	pub async fn children<H>(&self, handle: &H) -> tg::Result<Vec<tg::Object>>
	where
		H: tg::Handle,
	{
		let object = self.load(handle).await?;
		Ok(object.children())
	}

	pub async fn data<H>(&self, handle: &H) -> tg::Result<Data>
	where
		H: tg::Handle,
	{
		Ok(self.object(handle).await?.to_data())
	}
}

impl Directory {
	#[must_use]
	pub fn with_entries(entries: BTreeMap<String, tg::Artifact>) -> Self {
		Self::with_object(Object::Node(tg::directory::object::Node { entries }))
	}

	#[must_use]
	pub fn with_graph_and_node(graph: tg::Graph, node: usize) -> Self {
		Self::with_object(Object::Graph(tg::directory::object::Graph { graph, node }))
	}

	pub async fn builder<H>(&self, handle: &H) -> tg::Result<Builder>
	where
		H: tg::Handle,
	{
		let entries = self.entries(handle).await?;
		let builder = Builder::with_entries(entries);
		Ok(builder)
	}

	pub async fn entries<H>(&self, handle: &H) -> tg::Result<BTreeMap<String, tg::Artifact>>
	where
		H: tg::Handle,
	{
		let object = self.object(handle).await?;
		let entries = match object.as_ref() {
			Object::Graph(object) => {
				let graph = &object.graph;
				let node = object.node;
				let object = graph.object(handle).await?;
				let node = object
					.nodes
					.get(node)
					.ok_or_else(|| tg::error!("invalid index"))?;
				let directory = node
					.try_unwrap_directory_ref()
					.ok()
					.ok_or_else(|| tg::error!("expected a directory"))?;
				directory
					.entries
					.iter()
					.map(|(name, either)| {
						let artifact = match either {
							Either::Left(node) => {
								let kind = object
									.nodes
									.get(*node)
									.ok_or_else(|| tg::error!("invalid index"))?
									.kind();
								match kind {
									tg::artifact::Kind::Directory => {
										tg::Directory::with_graph_and_node(graph.clone(), *node)
											.into()
									},
									tg::artifact::Kind::File => {
										tg::File::with_graph_and_node(graph.clone(), *node).into()
									},
									tg::artifact::Kind::Symlink => {
										tg::Symlink::with_graph_and_node(graph.clone(), *node)
											.into()
									},
								}
							},
							Either::Right(artifact) => artifact.clone(),
						};
						Ok((name.clone(), artifact))
					})
					.collect::<tg::Result<_>>()?
			},
			Object::Node(node) => node.entries.clone(),
		};
		Ok(entries)
	}

	pub async fn try_get_entry<H>(&self, handle: &H, name: &str) -> tg::Result<Option<tg::Artifact>>
	where
		H: tg::Handle,
	{
		let object = self.object(handle).await?;
		let artifact = match object.as_ref() {
			Object::Graph(object) => {
				let graph = &object.graph;
				let node = object.node;
				let object = graph.object(handle).await?;
				let node = object
					.nodes
					.get(node)
					.ok_or_else(|| tg::error!("invalid index"))?;
				let directory = node
					.try_unwrap_directory_ref()
					.ok()
					.ok_or_else(|| tg::error!("expected a directory"))?;
				match directory.entries.get(name) {
					None => None,
					Some(Either::Left(node)) => {
						let kind = object
							.nodes
							.get(*node)
							.ok_or_else(|| tg::error!("invalid index"))?
							.kind();
						let artifact = match kind {
							tg::artifact::Kind::Directory => {
								tg::Directory::with_graph_and_node(graph.clone(), *node).into()
							},
							tg::artifact::Kind::File => {
								tg::File::with_graph_and_node(graph.clone(), *node).into()
							},
							tg::artifact::Kind::Symlink => {
								tg::Symlink::with_graph_and_node(graph.clone(), *node).into()
							},
						};
						Some(artifact)
					},
					Some(Either::Right(artifact)) => Some(artifact.clone()),
				}
			},
			Object::Node(node) => node.entries.get(name).cloned(),
		};
		Ok(artifact)
	}

	pub async fn get<H>(&self, handle: &H, path: impl AsRef<Path>) -> tg::Result<tg::Artifact>
	where
		H: tg::Handle,
	{
		let artifact = self
			.try_get(handle, path)
			.await?
			.ok_or_else(|| tg::error!("failed to get the artifact"))?;
		Ok(artifact)
	}

	pub async fn try_get<H>(
		&self,
		handle: &H,
		path: impl AsRef<Path>,
	) -> tg::Result<Option<tg::Artifact>>
	where
		H: tg::Handle,
	{
		let mut path = path.as_ref().to_owned();

		// Track the current artifact.
		let mut artifact: tg::Artifact = self.clone().into();

		// Track the parent directories.
		let mut parents: Vec<tg::Directory> = vec![];

		// Handle each path component.
		loop {
			// Handle the first path component.
			let Some(component) = path.components().next() else {
				break;
			};
			let name = match component {
				// Prefix and root components are not allowed.
				std::path::Component::Prefix(_) | std::path::Component::RootDir => {
					return Err(tg::error!("invalid path"));
				},

				// Ignore current components.
				std::path::Component::CurDir => {
					path = path.components().skip(1).collect();
					continue;
				},

				// If the component is a parent component, then remove the last parent and continue.
				std::path::Component::ParentDir => {
					path = path.components().skip(1).collect();
					artifact = parents
						.pop()
						.ok_or_else(|| tg::error!("the path is external"))?
						.into();
					continue;
				},

				std::path::Component::Normal(name) => {
					let name = name
						.to_str()
						.ok_or_else(|| tg::error!("invalid path"))?
						.to_owned();
					path = path.components().skip(1).collect();
					name
				},
			};

			// Get the artifact. If it doesn't exist, then return `None`.
			let directory = artifact
				.try_unwrap_directory()
				.ok()
				.ok_or_else(|| tg::error!("the path is external"))?;
			let Some(entry) = directory.try_get_entry(handle, &name).await? else {
				return Ok(None);
			};
			parents.push(directory.clone());
			artifact = entry;

			// Handle a symlink.
			if let tg::Artifact::Symlink(symlink) = &artifact {
				let mut artifact_ = symlink.artifact(handle).await?.clone();
				if let Some(tg::Artifact::Symlink(symlink)) = artifact_ {
					artifact_ = Box::pin(symlink.try_resolve(handle)).await?;
				}
				let path_ = symlink.path(handle).await?.clone();
				match (artifact_, path_) {
					(None, Some(path_)) => {
						artifact = parents
							.pop()
							.ok_or_else(|| tg::error!("the path is external"))?
							.into();
						path = path_.join(path);
					},
					(Some(artifact), None) => {
						return Ok(Some(artifact));
					},
					(Some(tg::Artifact::Directory(directory)), Some(path)) => {
						return Box::pin(directory.try_get(handle, path)).await;
					},
					_ => {
						return Err(tg::error!("invalid symlink"));
					},
				}
			}
		}

		Ok(Some(artifact))
	}
}

impl std::fmt::Display for Directory {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let mut printer = tg::value::print::Printer::new(f, tg::value::print::Options::default());
		printer.directory(self)?;
		Ok(())
	}
}

#[macro_export]
macro_rules! directory {
	{ $($name:expr => $artifact:expr),* $(,)? } => {{
		let mut entries = ::std::collections::BTreeMap::new();
		$(
			entries.insert($name.into(), $artifact.into());
		)*
		$crate::Directory::with_entries(entries)
	}};
}
