use crate::{self as tg, util::http::full};
use http_body_util::BodyExt as _;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Arg {
	pub blob: tg::blob::Id,
	pub format: tg::artifact::archive::Format,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Output {
	pub id: tg::artifact::Id,
}

impl tg::Artifact {
	pub async fn extract<H>(
		handle: &H,
		blob: &tg::Blob,
		format: tg::artifact::archive::Format,
	) -> tg::Result<Self>
	where
		H: tg::Handle,
	{
		let blob = blob.id(handle, None).await?;
		let arg = Arg { blob, format };
		let output = handle.extract_artifact(arg).await?;
		let artifact = Self::with_id(output.id);
		Ok(artifact)
	}
}

impl tg::Client {
	pub async fn extract_artifact(
		&self,
		arg: tg::artifact::extract::Arg,
	) -> tg::Result<tg::artifact::extract::Output> {
		let method = http::Method::POST;
		let uri = "/artifacts/extract";
		let body = serde_json::to_string(&arg)
			.map_err(|source| tg::error!(!source, "failed to serialize the body"))?;
		let body = full(body);
		let request = http::request::Builder::default()
			.method(method)
			.uri(uri)
			.body(body)
			.map_err(|source| tg::error!(!source, "failed to create the request"))?;
		let response = self.send(request).await?;
		if !response.status().is_success() {
			let bytes = response
				.collect()
				.await
				.map_err(|source| tg::error!(!source, "failed to collect the response body"))?
				.to_bytes();
			let error = serde_json::from_slice(&bytes)
				.unwrap_or_else(|_| tg::error!("the request did not succeed"));
			return Err(error);
		}
		let bytes = response
			.collect()
			.await
			.map_err(|source| tg::error!(!source, "failed to collect the response body"))?
			.to_bytes();
		let output = serde_json::from_slice(&bytes)
			.map_err(|source| tg::error!(!source, "failed to deserialize the body"))?;
		Ok(output)
	}
}
