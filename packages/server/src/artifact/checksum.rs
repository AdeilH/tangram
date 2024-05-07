use crate::Server;
use tangram_client as tg;
use tangram_http::{
	incoming::RequestExt as _, outgoing::ResponseBuilderExt as _, Incoming, Outgoing,
};

impl Server {
	pub async fn checksum_artifact(
		&self,
		_id: &tg::artifact::Id,
		arg: tg::artifact::checksum::Arg,
	) -> tg::Result<tg::artifact::checksum::Output> {
		match arg.algorithm {
			tg::checksum::Algorithm::Unsafe => Ok(tg::Checksum::Unsafe),
			_ => Err(tg::error!("unimplemented")),
		}
	}
}

impl Server {
	pub(crate) async fn handle_checksum_artifact_request<H>(
		handle: &H,
		request: http::Request<Incoming>,
		id: &str,
	) -> tg::Result<http::Response<Outgoing>>
	where
		H: tg::Handle,
	{
		let id = id.parse()?;
		let arg = request.json().await?;
		let output = handle.checksum_artifact(&id, arg).await?;
		let response = http::Response::builder().json(output).unwrap();
		Ok(response)
	}
}
