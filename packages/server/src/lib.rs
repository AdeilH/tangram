use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use std::{
	collections::{BinaryHeap, HashMap},
	os::fd::AsRawFd,
	path::{Path, PathBuf},
	sync::Arc,
};
use tangram_client as tg;
use tangram_error::{Result, Wrap, WrapErr};
use tg::util::rmrf;

mod artifact;
mod build;
mod clean;
mod migrations;
mod object;
mod package;
mod serve;

/// A server.
#[derive(Clone)]
pub struct Server {
	inner: Arc<Inner>,
}

struct Inner {
	database: Database,
	file_descriptor_semaphore: tokio::sync::Semaphore,
	http: Task,
	local_builds: std::sync::RwLock<HashMap<tg::build::Id, LocalBuildState, fnv::FnvBuildHasher>>,
	local_queue: std::sync::Mutex<BinaryHeap<tg::build::queue::Item>>,
	local_queue_task: std::sync::Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
	local_queue_task_sender: tokio::sync::mpsc::UnboundedSender<LocalQueueTaskMessage>,
	local_task_pool_handle: tokio_util::task::LocalPoolHandle,
	lock: std::sync::Mutex<Option<tokio::fs::File>>,
	path: PathBuf,
	remote: Option<Box<dyn tg::Handle>>,
	remote_builds: std::sync::RwLock<HashMap<tg::build::Id, RemoteBuildState, fnv::FnvBuildHasher>>,
	remote_queue_task: std::sync::Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
	remote_queue_task_sender: tokio::sync::mpsc::UnboundedSender<()>,
	semaphore: Arc<tokio::sync::Semaphore>,
	store: Store,
	version: String,
	vfs: std::sync::Mutex<Option<tangram_vfs::Server>>,
}

struct Database {
	pool: tangram_pool::Pool<rusqlite::Connection>,
}

#[derive(Debug)]
struct Store {
	env: lmdb::Environment,
	objects: lmdb::Database,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct Build {
	children: Vec<tg::build::Id>,
	log: Option<tg::blob::Id>,
	outcome: Option<tg::build::outcome::Data>,
	status: tg::build::Status,
	target: tg::target::Id,
}

#[derive(Clone, Debug)]
struct LocalBuildState {
	inner: Arc<LocalBuildStateInner>,
}

#[derive(Debug)]
struct LocalBuildStateInner {
	children: std::sync::Mutex<ChildrenState>,
	item: tg::build::queue::Item,
	log: tokio::sync::Mutex<LogState>,
	outcome: OutcomeState,
	status: std::sync::Mutex<tg::build::Status>,
	task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Clone, Debug)]
struct RemoteBuildState {
	inner: Arc<RemoteBuildStateInner>,
}

#[derive(Debug)]
struct RemoteBuildStateInner {
	task: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Debug)]
struct ChildrenState {
	children: Vec<tg::build::Id>,
	sender: Option<tokio::sync::broadcast::Sender<tg::build::Id>>,
}

#[derive(Debug)]
struct LogState {
	file: tokio::fs::File,
	sender: Option<tokio::sync::broadcast::Sender<Bytes>>,
}

#[derive(Debug)]
struct OutcomeState {
	sender: tokio::sync::watch::Sender<Option<tg::build::Outcome>>,
}

enum LocalQueueTaskMessage {
	Added,
	Finished,
	Stop,
}

type Task = (
	std::sync::Mutex<Option<tokio::task::JoinHandle<Result<()>>>>,
	std::sync::Mutex<Option<tokio::task::AbortHandle>>,
);

pub struct Options {
	pub addr: tg::client::Addr,
	pub build: Option<BuildOptions>,
	pub path: PathBuf,
	pub remote: Option<RemoteOptions>,
	pub version: String,
}

pub struct BuildOptions {
	pub remote: Option<RemoteBuildOptions>,
}

pub struct RemoteBuildOptions {
	pub enable: bool,
	pub hosts: Option<Vec<tg::System>>,
}

pub struct RemoteOptions {
	pub tg: Box<dyn tg::Handle>,
}

impl Server {
	#[allow(clippy::too_many_lines)]
	pub async fn start(options: Options) -> Result<Server> {
		// Get the addr.
		let addr = options.addr;

		// Get the path.
		let path = options.path;

		// Ensure the path exists.
		tokio::fs::create_dir_all(&path)
			.await
			.wrap_err("Failed to create the directory.")?;

		// Acquire an exclusive lock to the path.
		let lock = tokio::fs::OpenOptions::new()
			.read(true)
			.write(true)
			.create(true)
			.open(path.join("lock"))
			.await
			.wrap_err("Failed to open the lock file.")?;
		let ret = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
		if ret != 0 {
			return Err(std::io::Error::last_os_error().wrap("Failed to acquire the lock file."));
		}
		let lock = std::sync::Mutex::new(Some(lock));

		// Migrate the path.
		Self::migrate(&path).await?;

		// Write the PID file.
		tokio::fs::write(&path.join("server.pid"), std::process::id().to_string())
			.await
			.wrap_err("Failed to write the PID file.")?;

		// Remove an existing socket file.
		rmrf(&path.join("socket"))
			.await
			.wrap_err("Failed to remove an existing socket file.")?;

		// Create the semaphore.
		let semaphore = Arc::new(tokio::sync::Semaphore::new(
			std::thread::available_parallelism().unwrap().get(),
		));

		// Create the local builds.
		let local_builds = std::sync::RwLock::new(HashMap::default());

		// Create the local queue.
		let local_queue = std::sync::Mutex::new(BinaryHeap::new());

		// Create the local queue task.
		let local_queue_task = std::sync::Mutex::new(None);

		// Create the local queue task channel.
		let (local_queue_task_sender, queue_task_receiver) = tokio::sync::mpsc::unbounded_channel();

		// Create the remote builds.
		let remote_builds = std::sync::RwLock::new(HashMap::default());

		// Create the remote queue task.
		let remote_queue_task = std::sync::Mutex::new(None);

		// Create the remote queue task channel.
		let (remote_queue_task_sender, remote_queue_task_receiver) =
			tokio::sync::mpsc::unbounded_channel();

		// Open the database.
		let database_path = path.join("database");
		let pool = tangram_pool::Pool::new();
		let n = std::thread::available_parallelism().unwrap().get();
		for _ in 0..n {
			let db = rusqlite::Connection::open(&database_path)
				.wrap_err("Failed to open the database.")?;
			pool.put(db).await;
		}
		let database = Database { pool };

		// Create the file system semaphore.
		let file_descriptor_semaphore = tokio::sync::Semaphore::new(16);

		// Create the HTTP server task.
		let http = (std::sync::Mutex::new(None), std::sync::Mutex::new(None));

		// Create the local pool.
		let local_task_pool_handle = tokio_util::task::LocalPoolHandle::new(
			std::thread::available_parallelism().unwrap().get(),
		);

		// Get the remote.
		let remote = if let Some(remote) = options.remote {
			Some(remote.tg)
		} else {
			None
		};

		// Open the store.
		let store_path = path.join("store");
		let env = lmdb::Environment::new()
			.set_map_size(1_099_511_627_776)
			.set_max_dbs(1)
			.set_max_readers(1024)
			.set_flags(lmdb::EnvironmentFlags::NO_SUB_DIR)
			.open(&store_path)
			.wrap_err("Failed to open the store.")?;
		let objects = env
			.open_db(Some("objects"))
			.wrap_err("Failed to open the objects database.")?;
		let store = Store { env, objects };

		// Get the version.
		let version = options.version;

		// Create the VFS.
		let vfs = std::sync::Mutex::new(None);

		// Create the server.
		let inner = Arc::new(Inner {
			database,
			file_descriptor_semaphore,
			http,
			local_builds,
			local_queue,
			local_queue_task,
			local_queue_task_sender,
			local_task_pool_handle,
			lock,
			path,
			remote,
			remote_builds,
			remote_queue_task,
			remote_queue_task_sender,
			semaphore,
			store,
			version,
			vfs,
		});
		let server = Server { inner };

		// Start the VFS server.
		let vfs = tangram_vfs::Server::start(&server, &server.artifacts_path())
			.await
			.wrap_err("Failed to start the VFS server.")?;
		server.inner.vfs.lock().unwrap().replace(vfs);

		// Start the build queue task.
		server
			.inner
			.local_queue_task
			.lock()
			.unwrap()
			.replace(tokio::spawn({
				let server = server.clone();
				async move {
					match server.local_queue_task(queue_task_receiver).await {
						Ok(()) => Ok(()),
						Err(error) => {
							tracing::error!(?error);
							Err(error)
						},
					}
				}
			}));

		// Start the build queue remote task.
		server
			.inner
			.remote_queue_task
			.lock()
			.unwrap()
			.replace(tokio::spawn({
				let server = server.clone();
				async move {
					match server.remote_queue_task(remote_queue_task_receiver).await {
						Ok(()) => Ok(()),
						Err(error) => {
							tracing::error!(?error);
							Err(error)
						},
					}
				}
			}));

		// Start the HTTP server task.
		let task = tokio::spawn({
			let server = server.clone();
			async move {
				match server.serve(addr).await {
					Ok(()) => Ok(()),
					Err(error) => {
						tracing::error!(?error);
						Err(error)
					},
				}
			}
		});
		let abort = task.abort_handle();
		server.inner.http.0.lock().unwrap().replace(task);
		server.inner.http.1.lock().unwrap().replace(abort);

		Ok(server)
	}

	#[allow(clippy::unused_async, clippy::unnecessary_wraps)]
	pub async fn stop(&self) -> Result<()> {
		// Stop the http server task.
		if let Some(handle) = self.inner.http.1.lock().unwrap().as_ref() {
			handle.abort();
		};

		// Stop the build queue remote task.
		self.inner.remote_queue_task_sender.send(()).ok();

		// Stop the build queue task.
		self.inner
			.local_queue_task_sender
			.send(LocalQueueTaskMessage::Stop)
			.unwrap();

		Ok(())
	}

	pub async fn join(&self) -> Result<()> {
		// Join the HTTP server task.
		let task = self.inner.http.0.lock().unwrap().take();
		if let Some(task) = task {
			match task.await {
				Ok(result) => Ok(result),
				Err(error) if error.is_cancelled() => Ok(Ok(())),
				Err(error) => Err(error),
			}
			.unwrap()?;
		}

		// Join the build queue remote task.
		let build_queue_remote_task = self.inner.remote_queue_task.lock().unwrap().take().unwrap();
		build_queue_remote_task.await.unwrap()?;

		// Join the build queue task.
		let build_queue_task = self.inner.local_queue_task.lock().unwrap().take().unwrap();
		build_queue_task.await.unwrap()?;

		// Join the VFS server.
		let vfs = self.inner.vfs.lock().unwrap().clone();
		if let Some(vfs) = vfs {
			vfs.stop();
			vfs.join().await?;
		}

		// Release the lock.
		self.inner.lock.lock().unwrap().take();

		Ok(())
	}

	#[allow(clippy::unused_async)]
	async fn status(&self) -> Result<tg::status::Status> {
		Ok(tg::status::Status {
			version: self.inner.version.clone(),
		})
	}

	#[must_use]
	pub fn path(&self) -> &Path {
		&self.inner.path
	}

	#[must_use]
	pub fn artifacts_path(&self) -> PathBuf {
		self.path().join("artifacts")
	}

	#[must_use]
	pub fn database_path(&self) -> PathBuf {
		self.path().join("database")
	}

	#[must_use]
	pub fn store_path(&self) -> PathBuf {
		self.path().join("store")
	}

	#[must_use]
	pub fn tmp_path(&self) -> PathBuf {
		self.path().join("tmp")
	}
}

#[async_trait]
impl tg::Handle for Server {
	fn clone_box(&self) -> Box<dyn tg::Handle> {
		Box::new(self.clone())
	}

	fn file_descriptor_semaphore(&self) -> &tokio::sync::Semaphore {
		&self.inner.file_descriptor_semaphore
	}

	async fn status(&self) -> Result<tg::status::Status> {
		self.status().await
	}

	async fn stop(&self) -> Result<()> {
		self.stop().await?;
		Ok(())
	}

	async fn clean(&self) -> Result<()> {
		self.clean().await
	}

	async fn get_object_exists(&self, id: &tg::object::Id) -> Result<bool> {
		self.get_object_exists(id).await
	}

	async fn try_get_object(&self, id: &tg::object::Id) -> Result<Option<Bytes>> {
		self.try_get_object(id).await
	}

	async fn try_put_object(
		&self,
		id: &tg::object::Id,
		bytes: &Bytes,
	) -> Result<Vec<tg::object::Id>> {
		self.try_put_object(id, bytes).await
	}

	async fn push_object(&self, id: &tg::object::Id) -> Result<()> {
		self.push_object(id).await
	}

	async fn pull_object(&self, id: &tg::object::Id) -> Result<()> {
		self.pull_object(id).await
	}

	async fn check_in_artifact(&self, path: &tg::Path) -> Result<tg::artifact::Id> {
		self.check_in_artifact(path).await
	}

	async fn check_out_artifact(&self, id: &tg::artifact::Id, path: &tg::Path) -> Result<()> {
		self.check_out_artifact(id, path).await
	}

	async fn try_get_assignment(&self, id: &tg::target::Id) -> Result<Option<tg::build::Id>> {
		self.try_get_assignment(id).await
	}

	async fn get_or_create_build(
		&self,
		user: Option<&tg::User>,
		id: &tg::target::Id,
		depth: u64,
		retry: tg::build::Retry,
	) -> Result<tg::build::Id> {
		self.get_or_create_build(user, id, depth, retry).await
	}

	async fn try_get_queue_item(
		&self,
		user: Option<&tg::User>,
		hosts: Option<Vec<tg::System>>,
	) -> Result<Option<tg::build::queue::Item>> {
		self.try_get_queue_item(user, hosts).await
	}

	async fn try_get_build_status(&self, id: &tg::build::Id) -> Result<Option<tg::build::Status>> {
		self.try_get_build_status(id).await
	}

	async fn try_get_build_target(&self, id: &tg::build::Id) -> Result<Option<tg::target::Id>> {
		self.try_get_build_target(id).await
	}

	async fn try_get_build_children(
		&self,
		id: &tg::build::Id,
	) -> Result<Option<BoxStream<'static, Result<tg::build::Id>>>> {
		self.try_get_build_children(id).await
	}

	async fn add_build_child(
		&self,
		user: Option<&tg::User>,
		build_id: &tg::build::Id,
		child_id: &tg::build::Id,
	) -> Result<()> {
		self.add_build_child(user, build_id, child_id).await
	}

	async fn try_get_build_log(
		&self,
		id: &tg::build::Id,
	) -> Result<Option<BoxStream<'static, Result<Bytes>>>> {
		self.try_get_build_log(id).await
	}

	async fn add_build_log(
		&self,
		user: Option<&tg::User>,
		build_id: &tg::build::Id,
		bytes: Bytes,
	) -> Result<()> {
		self.add_build_log(user, build_id, bytes).await
	}

	async fn try_get_build_outcome(
		&self,
		id: &tg::build::Id,
	) -> Result<Option<tg::build::Outcome>> {
		self.try_get_build_outcome(id).await
	}

	async fn cancel_build(&self, user: Option<&tg::User>, id: &tg::build::Id) -> Result<()> {
		self.cancel_build(user, id).await
	}

	async fn finish_build(
		&self,
		user: Option<&tg::User>,
		id: &tg::build::Id,
		outcome: tg::build::Outcome,
	) -> Result<()> {
		self.finish_build(user, id, outcome).await
	}

	async fn search_packages(&self, query: &str) -> Result<Vec<String>> {
		self.search_packages(query).await
	}

	async fn try_get_package(
		&self,
		dependency: &tg::Dependency,
	) -> Result<Option<tg::directory::Id>> {
		self.try_get_package(dependency).await
	}

	async fn try_get_package_and_lock(
		&self,
		dependency: &tg::Dependency,
	) -> Result<Option<(tg::directory::Id, tg::lock::Id)>> {
		self.try_get_package_and_lock(dependency).await
	}

	async fn try_get_package_versions(
		&self,
		dependency: &tg::Dependency,
	) -> Result<Option<Vec<String>>> {
		self.try_get_package_versions(dependency).await
	}

	async fn try_get_package_metadata(
		&self,
		dependency: &tg::Dependency,
	) -> Result<Option<tg::package::Metadata>> {
		self.try_get_package_metadata(dependency).await
	}

	async fn try_get_package_dependencies(
		&self,
		dependency: &tg::Dependency,
	) -> Result<Option<Vec<tg::Dependency>>> {
		self.try_get_package_dependencies(dependency).await
	}

	async fn publish_package(&self, user: Option<&tg::User>, id: &tg::directory::Id) -> Result<()> {
		self.publish_package(user, id).await
	}

	async fn create_login(&self) -> Result<tg::user::Login> {
		self.inner
			.remote
			.as_ref()
			.wrap_err("The server does not have a remote.")?
			.create_login()
			.await
	}

	async fn get_login(&self, id: &tg::Id) -> Result<Option<tg::user::Login>> {
		self.inner
			.remote
			.as_ref()
			.wrap_err("The server does not have a remote.")?
			.get_login(id)
			.await
	}

	async fn get_user_for_token(&self, token: &str) -> Result<Option<tg::user::User>> {
		self.inner
			.remote
			.as_ref()
			.wrap_err("The server does not have a remote.")?
			.get_user_for_token(token)
			.await
	}
}
