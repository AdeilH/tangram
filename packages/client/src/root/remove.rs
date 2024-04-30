use crate as tg;
use tangram_http::{incoming::ResponseExt as _, Outgoing};

impl tg::Client {
	pub async fn remove_root(&self, name: &str) -> tg::Result<()> {
		let method = http::Method::DELETE;
		let name = urlencoding::encode(name);
		let uri = format!("/roots/{name}");
		let request = http::request::Builder::default()
			.method(method)
			.uri(uri)
			.body(Outgoing::empty())
			.unwrap();
		let response = self.send(request).await?;
		if !response.status().is_success() {
			let error = response.json().await?;
			return Err(error);
		}
		Ok(())
	}
}