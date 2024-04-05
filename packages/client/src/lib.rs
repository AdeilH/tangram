use self::util::http::{Incoming, Outgoing};
pub use self::{
	artifact::Artifact,
	blob::Blob,
	branch::Branch,
	build::Build,
	checksum::Checksum,
	dependency::Dependency,
	diagnostic::Diagnostic,
	directory::Directory,
	document::Document,
	error::{ok, Error, Result},
	file::File,
	handle::Handle,
	id::Id,
	import::Import,
	leaf::Leaf,
	location::Location,
	lock::Lock,
	module::Module,
	mutation::Mutation,
	object::Handle as Object,
	path::Path,
	position::Position,
	range::Range,
	server::Health,
	symlink::Symlink,
	target::Target,
	template::Template,
	user::Login,
	user::User,
	value::Value,
};
use crate as tg;
use bytes::Bytes;
use futures::stream::BoxStream;
use std::sync::Arc;
use tokio::{
	io::{AsyncRead, AsyncWrite},
	net::{TcpStream, UnixStream},
};
use url::Url;

pub mod artifact;
pub mod blob;
pub mod branch;
pub mod build;
pub mod bundle;
pub mod checksum;
pub mod dependency;
pub mod diagnostic;
pub mod directory;
pub mod document;
pub mod error;
pub mod file;
pub mod handle;
pub mod id;
pub mod import;
pub mod language;
pub mod leaf;
pub mod location;
pub mod lock;
pub mod module;
pub mod mutation;
pub mod object;
pub mod package;
pub mod path;
pub mod position;
pub mod range;
pub mod server;
pub mod symlink;
pub mod target;
pub mod template;
pub mod user;
mod util;
pub mod value;

#[derive(Debug, Clone)]
pub struct Client {
	inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
	url: Url,
	file_descriptor_semaphore: tokio::sync::Semaphore,
	sender: tokio::sync::Mutex<Option<hyper::client::conn::http2::SendRequest<Outgoing>>>,
	user: Option<User>,
}

pub struct Builder {
	url: Url,
	user: Option<User>,
}

impl Client {
	pub fn with_env() -> tg::Result<Self> {
		let url = std::env::var("TANGRAM_URL")
			.map_err(|error| {
				error!(
					source = error,
					"failed to get the TANGRAM_URL environment variable"
				)
			})?
			.parse()
			.map_err(|error| {
				error!(
					source = error,
					"could not parse a URL from the TANGRAM_URL environment variable"
				)
			})?;
		let file_descriptor_semaphore = tokio::sync::Semaphore::new(32);
		let sender = tokio::sync::Mutex::new(None);
		let user = None;
		let inner = Arc::new(Inner {
			url,
			file_descriptor_semaphore,
			sender,
			user,
		});
		let client = Client { inner };
		Ok(client)
	}

	pub async fn connect(&self) -> tg::Result<()> {
		self.sender().await.map(|_| ())
	}

	pub async fn disconnect(&self) -> tg::Result<()> {
		self.inner.sender.lock().await.take();
		Ok(())
	}

	async fn sender(&self) -> tg::Result<hyper::client::conn::http2::SendRequest<Outgoing>> {
		if let Some(sender) = self.inner.sender.lock().await.as_ref().cloned() {
			if sender.is_ready() {
				return Ok(sender);
			}
		}
		let mut sender_guard = self.inner.sender.lock().await;
		let sender = self.connect_h2().await?;
		sender_guard.replace(sender.clone());
		Ok(sender)
	}

	async fn connect_h1(&self) -> tg::Result<hyper::client::conn::http1::SendRequest<Outgoing>> {
		match self.inner.url.scheme() {
			"unix" => {
				let path = std::path::Path::new(self.inner.url.path());
				self.connect_unix_h1(path).await
			},
			"http" => {
				let host = self
					.inner
					.url
					.domain()
					.ok_or_else(|| error!("invalid url"))?;
				let port = self
					.inner
					.url
					.port_or_known_default()
					.ok_or_else(|| error!("invalid url"))?;
				self.connect_tcp_h1(host, port).await
			},
			"https" => {
				#[cfg(not(feature = "tls"))]
				{
					Err(error!("tls is not enabled"))
				}
				#[cfg(feature = "tls")]
				{
					let host = self
						.inner
						.url
						.domain()
						.ok_or_else(|| error!("invalid url"))?;
					let port = self
						.inner
						.url
						.port_or_known_default()
						.ok_or_else(|| error!("invalid url"))?;
					self.connect_tcp_tls_h1(host, port).await
				}
			},
			_ => Err(error!("invalid url")),
		}
	}

	async fn connect_h2(&self) -> tg::Result<hyper::client::conn::http2::SendRequest<Outgoing>> {
		match self.inner.url.scheme() {
			"unix" => {
				let path = std::path::Path::new(self.inner.url.path());
				self.connect_unix_h2(path).await
			},
			"http" => {
				let host = self
					.inner
					.url
					.domain()
					.ok_or_else(|| error!("invalid url"))?;
				let port = self
					.inner
					.url
					.port_or_known_default()
					.ok_or_else(|| error!("invalid url"))?;
				self.connect_tcp_h2(host, port).await
			},
			"https" => {
				#[cfg(not(feature = "tls"))]
				{
					Err(error!("tls is not enabled"))
				}
				#[cfg(feature = "tls")]
				{
					let host = self
						.inner
						.url
						.domain()
						.ok_or_else(|| error!("invalid url"))?;
					let port = self
						.inner
						.url
						.port_or_known_default()
						.ok_or_else(|| error!("invalid url"))?;
					self.connect_tcp_tls_h2(host, port).await
				}
			},
			_ => Err(error!("invalid url")),
		}
	}

	async fn connect_unix_h1(
		&self,
		path: &std::path::Path,
	) -> tg::Result<hyper::client::conn::http1::SendRequest<Outgoing>> {
		// Connect via UNIX.
		let stream = UnixStream::connect(path)
			.await
			.map_err(|source| error!(!source, "failed to connect to the socket"))?;

		// Perform the HTTP handshake.
		let io = hyper_util::rt::TokioIo::new(stream);
		let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
			.await
			.map_err(|source| error!(!source, "failed to perform the HTTP handshake"))?;

		// Spawn the connection.
		tokio::spawn(async move {
			connection
				.with_upgrades()
				.await
				.inspect_err(|error| {
					tracing::error!(error = ?error, "the connection failed");
				})
				.ok();
		});

		// Wait for the sender to be ready.
		sender
			.ready()
			.await
			.map_err(|source| error!(!source, "failed to ready the sender"))?;

		Ok(sender)
	}

	async fn connect_unix_h2(
		&self,
		path: &std::path::Path,
	) -> tg::Result<hyper::client::conn::http2::SendRequest<Outgoing>> {
		// Connect via UNIX.
		let stream = UnixStream::connect(path)
			.await
			.map_err(|source| error!(!source, "failed to connect to the socket"))?;

		// Perform the HTTP handshake.
		let executor = hyper_util::rt::TokioExecutor::new();
		let io = hyper_util::rt::TokioIo::new(stream);
		let (mut sender, connection) = hyper::client::conn::http2::handshake(executor, io)
			.await
			.map_err(|source| error!(!source, "failed to perform the HTTP handshake"))?;

		// Spawn the connection.
		tokio::spawn(async move {
			connection
				.await
				.inspect_err(|error| {
					tracing::error!(error = ?error, "the connection failed");
				})
				.ok();
		});

		// Wait for the sender to be ready.
		sender
			.ready()
			.await
			.map_err(|source| error!(!source, "failed to ready the sender"))?;

		Ok(sender)
	}

	async fn connect_tcp_h1(
		&self,
		host: &str,
		port: u16,
	) -> tg::Result<hyper::client::conn::http1::SendRequest<Outgoing>> {
		// Connect via TCP.
		let stream = TcpStream::connect(format!("{host}:{port}"))
			.await
			.map_err(|source| error!(!source, "failed to create the TCP connection"))?;

		// Perform the HTTP handshake.
		let io = hyper_util::rt::TokioIo::new(stream);
		let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
			.await
			.map_err(|source| error!(!source, "failed to perform the HTTP handshake"))?;

		// Spawn the connection.
		tokio::spawn(async move {
			connection
				.with_upgrades()
				.await
				.inspect_err(|error| {
					tracing::error!(error = ?error, "the connection failed");
				})
				.ok();
		});

		// Wait for the sender to be ready.
		sender
			.ready()
			.await
			.map_err(|source| error!(!source, "failed to ready the sender"))?;

		Ok(sender)
	}

	async fn connect_tcp_h2(
		&self,
		host: &str,
		port: u16,
	) -> tg::Result<hyper::client::conn::http2::SendRequest<Outgoing>> {
		// Connect via TCP.
		let stream = TcpStream::connect(format!("{host}:{port}"))
			.await
			.map_err(|source| error!(!source, "failed to create the TCP connection"))?;

		// Perform the HTTP handshake.
		let executor = hyper_util::rt::TokioExecutor::new();
		let io = hyper_util::rt::TokioIo::new(stream);
		let (mut sender, connection) = hyper::client::conn::http2::handshake(executor, io)
			.await
			.map_err(|source| error!(!source, "failed to perform the HTTP handshake"))?;

		// Spawn the connection.
		tokio::spawn(async move {
			connection
				.await
				.inspect_err(|error| {
					tracing::error!(error = ?error, "the connection failed");
				})
				.ok();
		});

		// Wait for the sender to be ready.
		sender
			.ready()
			.await
			.map_err(|source| error!(!source, "failed to ready the sender"))?;

		Ok(sender)
	}

	#[cfg(feature = "tls")]
	async fn connect_tcp_tls_h1(
		&self,
		host: &str,
		port: u16,
	) -> tg::Result<hyper::client::conn::http1::SendRequest<Outgoing>> {
		// Connect via TLS over TCP.
		let stream = self.connect_tcp_tls(host, port).await?;

		// Perform the HTTP handshake.
		let io = hyper_util::rt::TokioIo::new(stream);
		let (mut sender, connection) = hyper::client::conn::http1::handshake(io)
			.await
			.map_err(|source| error!(!source, "failed to perform the HTTP handshake"))?;

		// Spawn the connection.
		tokio::spawn(async move {
			connection
				.with_upgrades()
				.await
				.inspect_err(|error| {
					tracing::error!(error = ?error, "the connection failed");
				})
				.ok();
		});

		// Wait for the sender to be ready.
		sender
			.ready()
			.await
			.map_err(|source| error!(!source, "failed to ready the sender"))?;

		Ok(sender)
	}

	#[cfg(feature = "tls")]
	async fn connect_tcp_tls_h2(
		&self,
		host: &str,
		port: u16,
	) -> tg::Result<hyper::client::conn::http2::SendRequest<Outgoing>> {
		// Connect via TLS over TCP.
		let stream = self.connect_tcp_tls(host, port).await?;

		// Perform the HTTP handshake.
		let executor = hyper_util::rt::TokioExecutor::new();
		let io = hyper_util::rt::TokioIo::new(stream);
		let (mut sender, connection) = hyper::client::conn::http2::handshake(executor, io)
			.await
			.map_err(|source| error!(!source, "failed to perform the HTTP handshake"))?;

		// Spawn the connection.
		tokio::spawn(async move {
			connection
				.await
				.inspect_err(|error| {
					tracing::error!(error = ?error, "the connection failed");
				})
				.ok();
		});

		// Wait for the sender to be ready.
		sender
			.ready()
			.await
			.map_err(|source| error!(!source, "failed to ready the sender"))?;

		Ok(sender)
	}

	#[cfg(feature = "tls")]
	async fn connect_tcp_tls(
		&self,
		host: &str,
		port: u16,
	) -> tg::Result<tokio_rustls::client::TlsStream<tokio::net::TcpStream>> {
		// Connect via TCP.
		let stream = TcpStream::connect(format!("{host}:{port}"))
			.await
			.map_err(|source| error!(!source, "failed to create the TCP connection"))?;

		// Create the connector.
		let mut root_store = rustls::RootCertStore::empty();
		root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
		let mut config = rustls::ClientConfig::builder()
			.with_root_certificates(root_store)
			.with_no_client_auth();
		config.alpn_protocols = vec!["h2".into()];
		let connector = tokio_rustls::TlsConnector::from(Arc::new(config));

		// Create the server name.
		let server_name = rustls::pki_types::ServerName::try_from(host.to_string().as_str())
			.map_err(|source| error!(!source, "failed to create the server name"))?
			.to_owned();

		// Connect via TLS.
		let stream = connector
			.connect(server_name, stream)
			.await
			.map_err(|source| error!(!source, "failed to connect"))?;

		// Verify the negotiated protocol.
		if !stream
			.get_ref()
			.1
			.alpn_protocol()
			.map_or(false, |protocol| protocol == b"h2")
		{
			return Err(error!("failed to negotiate the HTTP/2 protocol"));
		}

		Ok(stream)
	}

	async fn send(
		&self,
		request: http::request::Request<Outgoing>,
	) -> tg::Result<http::Response<Incoming>> {
		self.sender()
			.await?
			.send_request(request)
			.await
			.map_err(|source| error!(!source, "failed to send the request"))
	}
}

impl Builder {
	#[must_use]
	pub fn new(url: Url) -> Self {
		Self { url, user: None }
	}

	#[must_use]
	pub fn user(mut self, user: Option<User>) -> Self {
		self.user = user;
		self
	}

	#[must_use]
	pub fn build(self) -> Client {
		let url = self.url;
		let file_descriptor_semaphore = tokio::sync::Semaphore::new(16);
		let sender = tokio::sync::Mutex::new(None);
		let user = self.user;
		let inner = Arc::new(Inner {
			url,
			file_descriptor_semaphore,
			sender,
			user,
		});
		Client { inner }
	}
}

impl Handle for Client {
	async fn path(&self) -> tg::Result<Option<crate::Path>> {
		self.path().await
	}

	fn file_descriptor_semaphore(&self) -> &tokio::sync::Semaphore {
		&self.inner.file_descriptor_semaphore
	}

	async fn check_in_artifact(
		&self,
		arg: tg::artifact::CheckInArg,
	) -> tg::Result<tg::artifact::CheckInOutput> {
		self.check_in_artifact(arg).await
	}

	async fn check_out_artifact(
		&self,
		id: &tg::artifact::Id,
		arg: tg::artifact::CheckOutArg,
	) -> tg::Result<tg::artifact::CheckOutOutput> {
		self.check_out_artifact(id, arg).await
	}

	async fn list_builds(&self, arg: tg::build::ListArg) -> tg::Result<tg::build::ListOutput> {
		self.list_builds(arg).await
	}

	async fn try_get_build(
		&self,
		id: &tg::build::Id,
		arg: tg::build::GetArg,
	) -> tg::Result<Option<tg::build::GetOutput>> {
		self.try_get_build(id, arg).await
	}

	async fn put_build(
		&self,
		user: Option<&tg::User>,
		id: &tg::build::Id,
		arg: &tg::build::PutArg,
	) -> tg::Result<()> {
		self.put_build(user, id, arg).await
	}

	async fn push_build(&self, user: Option<&tg::User>, id: &tg::build::Id) -> tg::Result<()> {
		self.push_build(user, id).await
	}

	async fn pull_build(&self, id: &tg::build::Id) -> tg::Result<()> {
		self.pull_build(id).await
	}

	async fn get_or_create_build(
		&self,
		user: Option<&tg::User>,
		arg: tg::build::GetOrCreateArg,
	) -> tg::Result<tg::build::GetOrCreateOutput> {
		self.get_or_create_build(user, arg).await
	}

	async fn try_get_build_status(
		&self,
		id: &tg::build::Id,
		arg: tg::build::status::GetArg,
		stop: Option<tokio::sync::watch::Receiver<bool>>,
	) -> tg::Result<Option<BoxStream<'static, Result<tg::build::Status>>>> {
		self.try_get_build_status(id, arg, stop).await
	}

	async fn set_build_status(
		&self,
		user: Option<&tg::User>,
		id: &tg::build::Id,
		status: tg::build::Status,
	) -> tg::Result<()> {
		self.set_build_status(user, id, status).await
	}

	async fn try_get_build_children(
		&self,
		id: &tg::build::Id,
		arg: tg::build::children::GetArg,
		stop: Option<tokio::sync::watch::Receiver<bool>>,
	) -> tg::Result<Option<BoxStream<'static, Result<tg::build::children::Chunk>>>> {
		self.try_get_build_children(id, arg, stop).await
	}

	async fn add_build_child(
		&self,
		user: Option<&tg::User>,
		build_id: &tg::build::Id,
		child_id: &tg::build::Id,
	) -> tg::Result<()> {
		self.add_build_child(user, build_id, child_id).await
	}

	async fn try_get_build_log(
		&self,
		id: &tg::build::Id,
		arg: tg::build::log::GetArg,
		stop: Option<tokio::sync::watch::Receiver<bool>>,
	) -> tg::Result<Option<BoxStream<'static, Result<tg::build::log::Chunk>>>> {
		self.try_get_build_log(id, arg, stop).await
	}

	async fn add_build_log(
		&self,
		user: Option<&tg::User>,
		id: &tg::build::Id,
		bytes: Bytes,
	) -> tg::Result<()> {
		self.add_build_log(user, id, bytes).await
	}

	async fn try_get_build_outcome(
		&self,
		id: &tg::build::Id,
		arg: tg::build::outcome::GetArg,
		stop: Option<tokio::sync::watch::Receiver<bool>>,
	) -> tg::Result<Option<Option<tg::build::Outcome>>> {
		self.try_get_build_outcome(id, arg, stop).await
	}

	async fn set_build_outcome(
		&self,
		user: Option<&tg::User>,
		id: &tg::build::Id,
		outcome: tg::build::Outcome,
	) -> tg::Result<()> {
		self.set_build_outcome(user, id, outcome).await
	}

	async fn format(&self, text: String) -> tg::Result<String> {
		self.format(text).await
	}

	async fn lsp(
		&self,
		input: Box<dyn AsyncRead + Send + Unpin + 'static>,
		output: Box<dyn AsyncWrite + Send + Unpin + 'static>,
	) -> tg::Result<()> {
		self.lsp(input, output).await
	}

	async fn try_get_object(
		&self,
		id: &tg::object::Id,
	) -> tg::Result<Option<tg::object::GetOutput>> {
		self.try_get_object(id).await
	}

	async fn put_object(
		&self,
		id: &tg::object::Id,
		arg: &tg::object::PutArg,
	) -> tg::Result<tg::object::PutOutput> {
		self.put_object(id, arg).await
	}

	async fn push_object(&self, id: &tg::object::Id) -> tg::Result<()> {
		self.push_object(id).await
	}

	async fn pull_object(&self, id: &tg::object::Id) -> tg::Result<()> {
		self.pull_object(id).await
	}

	async fn search_packages(
		&self,
		arg: tg::package::SearchArg,
	) -> tg::Result<tg::package::SearchOutput> {
		self.search_packages(arg).await
	}

	async fn try_get_package(
		&self,
		dependency: &tg::Dependency,
		arg: tg::package::GetArg,
	) -> tg::Result<Option<tg::package::GetOutput>> {
		self.try_get_package(dependency, arg).await
	}

	async fn try_get_package_versions(
		&self,
		dependency: &tg::Dependency,
	) -> tg::Result<Option<Vec<String>>> {
		self.try_get_package_versions(dependency).await
	}

	async fn publish_package(
		&self,
		user: Option<&tg::User>,
		id: &tg::directory::Id,
	) -> tg::Result<()> {
		self.publish_package(user, id).await
	}

	async fn check_package(&self, dependency: &tg::Dependency) -> tg::Result<Vec<tg::Diagnostic>> {
		self.check_package(dependency).await
	}

	async fn format_package(&self, dependency: &tg::Dependency) -> tg::Result<()> {
		self.format_package(dependency).await
	}

	async fn get_package_outdated(
		&self,
		arg: &tg::Dependency,
	) -> tg::Result<tg::package::OutdatedOutput> {
		self.get_package_outdated(arg).await
	}

	async fn get_runtime_doc(&self) -> tg::Result<serde_json::Value> {
		self.get_runtime_doc().await
	}

	async fn try_get_package_doc(
		&self,
		dependency: &tg::Dependency,
	) -> tg::Result<Option<serde_json::Value>> {
		self.try_get_package_doc(dependency).await
	}

	async fn health(&self) -> tg::Result<tg::Health> {
		self.health().await
	}

	async fn clean(&self) -> tg::Result<()> {
		self.clean().await
	}

	async fn stop(&self) -> tg::Result<()> {
		self.stop().await
	}

	async fn create_login(&self) -> tg::Result<tg::user::Login> {
		self.create_login().await
	}

	async fn get_login(&self, id: &tg::Id) -> tg::Result<Option<tg::Login>> {
		self.get_login(id).await
	}

	async fn get_user_for_token(&self, token: &str) -> tg::Result<Option<tg::User>> {
		self.get_user_for_token(token).await
	}
}
