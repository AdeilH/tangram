use crate::{
	util::http::{bad_request, full, Incoming, Outgoing},
	Server,
};
use http_body_util::BodyExt as _;
use std::pin::Pin;
use tangram_client as tg;
use tokio::io::AsyncRead;

impl Server {
	pub async fn decompress_blob(
		&self,
		id: &tg::blob::Id,
		arg: tg::blob::decompress::Arg,
	) -> tg::Result<tg::blob::decompress::Output> {
		let blob = tg::Blob::with_id(id.clone());
		let reader = blob.reader(self).await?;
		let reader: Pin<Box<dyn AsyncRead + Send + 'static>> = match arg.format {
			tg::blob::compress::Format::Bz2 => {
				Box::pin(async_compression::tokio::bufread::BzDecoder::new(reader))
			},
			tg::blob::compress::Format::Gz => {
				Box::pin(async_compression::tokio::bufread::GzipDecoder::new(reader))
			},
			tg::blob::compress::Format::Xz => {
				Box::pin(async_compression::tokio::bufread::XzDecoder::new(reader))
			},
			tg::blob::compress::Format::Zstd => {
				Box::pin(async_compression::tokio::bufread::ZstdDecoder::new(reader))
			},
		};
		let blob = tg::Blob::with_reader(self, reader, None).await?;
		let id = blob.id(self, None).await?;
		let output = tg::blob::decompress::Output { id };
		Ok(output)
	}
}

impl Server {
	pub(crate) async fn handle_decompress_blob_request<H>(
		handle: &H,
		request: http::Request<Incoming>,
	) -> tg::Result<http::Response<Outgoing>>
	where
		H: tg::Handle,
	{
		let path_components: Vec<&str> = request.uri().path().split('/').skip(1).collect();
		let ["blobs", id, "decompress"] = path_components.as_slice() else {
			let path = request.uri().path();
			return Err(tg::error!(%path, "unexpected path"));
		};
		let Ok(id) = id.parse() else {
			return Ok(bad_request());
		};

		// Read the body.
		let bytes = request
			.into_body()
			.collect()
			.await
			.map_err(|source| tg::error!(!source, "failed to read the body"))?
			.to_bytes();
		let arg = serde_json::from_slice(&bytes)
			.map_err(|source| tg::error!(!source, "failed to deserialize the body"))?;

		// Decompress the blob.
		let output = handle.decompress_blob(&id, arg).await?;

		// Create the response.
		let body = serde_json::to_vec(&output)
			.map_err(|source| tg::error!(!source, "failed to serialize the response"))?;
		let response = http::Response::builder().body(full(body)).unwrap();

		Ok(response)
	}
}
