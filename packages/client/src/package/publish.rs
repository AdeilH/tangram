use crate::{self as tg, util::http::full};
use http_body_util::BodyExt as _;

impl tg::Client {
	pub async fn publish_package(&self, id: &tg::directory::Id) -> tg::Result<()> {
		let method = http::Method::POST;
		let uri = "/packages";
		let mut request = http::request::Builder::default().method(method).uri(uri);
		if let Some(token) = self.token.as_ref() {
			request = request.header(http::header::AUTHORIZATION, format!("Bearer {token}"));
		}
		let body = serde_json::to_vec(&id)
			.map_err(|source| tg::error!(!source, "failed to serialize the body"))?;
		let body = full(body);
		let request = request
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
				.unwrap_or_else(|_| tg::error!("failed to deserialize the error"));
			return Err(error);
		}
		Ok(())
	}
}
