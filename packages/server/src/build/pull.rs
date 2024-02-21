use crate::{Http, Server};
use tangram_client as tg;
use tangram_error::{error, Result, WrapErr};
use tangram_util::http::{bad_request, ok, Incoming, Outgoing};

impl Server {
	pub async fn pull_build(&self, id: &tg::build::Id) -> Result<()> {
		let remote = self
			.inner
			.remote
			.as_ref()
			.wrap_err("The server does not have a remote.")?;
		tg::Build::with_id(id.clone())
			.pull(self, remote.as_ref())
			.await
			.wrap_err("Failed to pull the build.")?;
		Ok(())
	}
}

impl Http {
	pub async fn handle_pull_build_request(
		&self,
		request: http::Request<Incoming>,
	) -> Result<http::Response<Outgoing>> {
		// Get the path params.
		let path_components: Vec<&str> = request.uri().path().split('/').skip(1).collect();
		let ["build", id, "pull"] = path_components.as_slice() else {
			return Err(error!("Unexpected path."));
		};
		let Ok(id) = id.parse() else {
			return Ok(bad_request());
		};

		// Pull the build.
		self.inner.tg.pull_build(&id).await?;

		Ok(ok())
	}
}