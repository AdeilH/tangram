use super::{Builder, Data, Id, Object};
use crate as tg;
use itertools::Itertools as _;
use std::{collections::BTreeMap, sync::Arc};
use tangram_either::Either;
use tokio::io::AsyncBufRead;

#[derive(Clone, Debug)]
pub struct File {
	state: Arc<std::sync::RwLock<State>>,
}

pub type State = tg::object::State<Id, Object>;

impl File {
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
		self.try_load(handle)
			.await?
			.ok_or_else(|| tg::error!("failed to load the object"))
	}

	pub async fn try_load<H>(&self, handle: &H) -> tg::Result<Option<Arc<Object>>>
	where
		H: tg::Handle,
	{
		if let Some(object) = self.state.read().unwrap().object.clone() {
			return Ok(Some(object));
		}
		let id = self.state.read().unwrap().id.clone().unwrap();
		let Some(output) = handle.try_get_object(&id.into()).await? else {
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

impl File {
	#[must_use]
	pub fn builder(contents: impl Into<tg::Blob>) -> Builder {
		Builder::new(contents)
	}

	#[must_use]
	pub fn with_contents(contents: impl Into<tg::Blob>) -> Self {
		Self::builder(contents).build()
	}

	#[must_use]
	pub fn with_graph_and_node(graph: tg::Graph, node: usize) -> Self {
		Self::with_object(Object::Graph(tg::file::object::Graph { graph, node }))
	}

	pub async fn contents<H>(&self, handle: &H) -> tg::Result<tg::Blob>
	where
		H: tg::Handle,
	{
		let object = self.object(handle).await?;
		match object.as_ref() {
			Object::Graph(object) => {
				let graph = &object.graph;
				let node = object.node;
				let object = graph.object(handle).await?;
				let node = object
					.nodes
					.get(node)
					.ok_or_else(|| tg::error!("invalid index"))?;
				let file = node
					.try_unwrap_file_ref()
					.ok()
					.ok_or_else(|| tg::error!("expected a file"))?;
				let contents = file.contents.clone();
				Ok(contents)
			},
			Object::Node(object) => Ok(object.contents.clone()),
		}
	}

	pub async fn dependencies<H>(
		&self,
		handle: &H,
	) -> tg::Result<BTreeMap<tg::Reference, tg::Referent<tg::Object>>>
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
				let file = node
					.try_unwrap_file_ref()
					.ok()
					.ok_or_else(|| tg::error!("expected a file"))?;
				file.dependencies
					.iter()
					.map(|(reference, referent)| {
						let item = match &referent.item {
							Either::Left(index) => {
								let node = object
									.nodes
									.get(*index)
									.ok_or_else(|| tg::error!("invalid index"))?;
								match node {
									tg::graph::Node::Directory(_) => {
										tg::Directory::with_graph_and_node(graph.clone(), *index)
											.into()
									},
									tg::graph::Node::File(_) => {
										tg::File::with_graph_and_node(graph.clone(), *index).into()
									},
									tg::graph::Node::Symlink(_) => {
										tg::Symlink::with_graph_and_node(graph.clone(), *index)
											.into()
									},
								}
							},
							Either::Right(object) => object.clone(),
						};
						let referent = tg::Referent {
							item,
							path: referent.path.clone(),
							tag: referent.tag.clone(),
						};
						Ok::<_, tg::Error>((reference.clone(), referent))
					})
					.try_collect()?
			},
			Object::Node(node) => node.dependencies.clone(),
		};
		Ok(entries)
	}

	pub async fn get_dependency<H>(
		&self,
		handle: &H,
		reference: &tg::Reference,
	) -> tg::Result<tg::Referent<tg::Object>>
	where
		H: tg::Handle,
	{
		self.try_get_dependency(handle, reference)
			.await?
			.ok_or_else(|| tg::error!("expected the dependency to exist"))
	}

	pub async fn try_get_dependency<H>(
		&self,
		handle: &H,
		reference: &tg::Reference,
	) -> tg::Result<Option<tg::Referent<tg::Object>>>
	where
		H: tg::Handle,
	{
		let object = self.object(handle).await?;
		let referent = match object.as_ref() {
			Object::Graph(object) => {
				let graph = &object.graph;
				let node = object.node;
				let object = graph.object(handle).await?;
				let node = object
					.nodes
					.get(node)
					.ok_or_else(|| tg::error!("invalid index"))?;
				let file = node
					.try_unwrap_file_ref()
					.ok()
					.ok_or_else(|| tg::error!("expected a file"))?;
				let Some(referent) = file.dependencies.get(reference) else {
					return Ok(None);
				};
				let item = match referent.item.clone() {
					Either::Left(index) => match object.nodes.get(index) {
						Some(tg::graph::Node::Directory(_)) => {
							tg::Directory::with_graph_and_node(graph.clone(), index).into()
						},
						Some(tg::graph::Node::File(_)) => {
							tg::File::with_graph_and_node(graph.clone(), index).into()
						},
						Some(tg::graph::Node::Symlink(_)) => {
							tg::Symlink::with_graph_and_node(graph.clone(), index).into()
						},
						None => return Err(tg::error!("invalid index")),
					},
					Either::Right(object) => object,
				};
				Some(tg::Referent {
					item,
					path: referent.path.clone(),
					tag: referent.tag.clone(),
				})
			},
			Object::Node(node) => node.dependencies.get(reference).cloned(),
		};
		Ok(referent)
	}

	pub async fn executable<H>(&self, handle: &H) -> tg::Result<bool>
	where
		H: tg::Handle,
	{
		let object = self.object(handle).await?;
		match object.as_ref() {
			Object::Graph(object) => {
				let graph = &object.graph;
				let node = object.node;
				let object = graph.object(handle).await?;
				let node = object
					.nodes
					.get(node)
					.ok_or_else(|| tg::error!("invalid index"))?;
				let file = node
					.try_unwrap_file_ref()
					.ok()
					.ok_or_else(|| tg::error!("expected a file"))?;
				Ok(file.executable)
			},
			Object::Node(node) => Ok(node.executable),
		}
	}

	pub async fn length<H>(&self, handle: &H) -> tg::Result<u64>
	where
		H: tg::Handle,
	{
		self.contents(handle).await?.length(handle).await
	}

	pub async fn read<H>(
		&self,
		handle: &H,
		arg: tg::blob::read::Arg,
	) -> tg::Result<impl AsyncBufRead + Send + use<H>>
	where
		H: tg::Handle,
	{
		self.contents(handle).await?.read(handle, arg).await
	}

	pub async fn bytes<H>(&self, handle: &H) -> tg::Result<Vec<u8>>
	where
		H: tg::Handle,
	{
		self.contents(handle).await?.bytes(handle).await
	}

	pub async fn text<H>(&self, handle: &H) -> tg::Result<String>
	where
		H: tg::Handle,
	{
		self.contents(handle).await?.text(handle).await
	}
}

impl From<tg::Blob> for File {
	fn from(value: tg::Blob) -> Self {
		Self::with_contents(value)
	}
}

impl From<String> for File {
	fn from(value: String) -> Self {
		Self::with_contents(value)
	}
}

impl From<&str> for File {
	fn from(value: &str) -> Self {
		Self::with_contents(value)
	}
}

impl std::fmt::Display for File {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let mut printer = tg::value::print::Printer::new(f, tg::value::print::Options::default());
		printer.file(self)?;
		Ok(())
	}
}

#[macro_export]
macro_rules! file {
	(@$builder:ident dependencies = $dependencies:expr $(, $($arg:tt)*)?) => {
		$builder = $builder.dependencies($dependencies);
		$crate::file!(@$builder $($($arg)*)?)
	};
	(@$builder:ident executable = $executable:expr $(, $($arg:tt)*)?) => {
		$builder = $builder.executable($executable);
		$crate::file!(@$builder $($($arg)*)?)
	};
	(@$builder:ident) => {};
	($contents:expr $(, $($arg:tt)*)?) => {{
		let mut builder = $crate::file::Builder::new($contents);
		$crate::file!(@builder $($($arg)*)?);
		builder.build()
	}};
}
