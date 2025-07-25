use crate as tg;
use tangram_http::{request::builder::Ext as _, response::Ext as _};
use url::Url;

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Arg {
	pub url: Url,
}

impl tg::Client {
	pub async fn put_remote(&self, name: &str, arg: tg::remote::put::Arg) -> tg::Result<()> {
		let method = http::Method::PUT;
		let uri = format!("/remotes/{name}");
		let request = http::request::Builder::default()
			.method(method)
			.uri(uri)
			.json(arg)
			.map_err(|source| tg::error!(!source, "failed to serialize the arg"))?
			.unwrap();
		let response = self.send(request).await?;
		if !response.status().is_success() {
			let error = response.json().await?;
			return Err(error);
		}
		Ok(())
	}
}
