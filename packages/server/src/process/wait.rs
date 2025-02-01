use crate::Server;
use futures::{stream, Future, StreamExt as _};
use tangram_client::{self as tg, handle::Ext};
use tangram_futures::{stream::TryExt as _, task::Stop};
use tangram_http::{incoming::request::Ext as _, Incoming, Outgoing};

impl Server {
	pub async fn wait_process_future(
		&self,
		id: &tg::process::Id,
	) -> tg::Result<
		impl Future<Output = tg::Result<Option<tg::process::wait::Output>>> + Send + 'static,
	> {
		let server = self.clone();
		let id = id.clone();
		Ok(async move {
			let stream = server.get_process_status(&id).await?.boxed();
			let status = stream
				.try_last()
				.await?
				.ok_or_else(|| tg::error!("failed to get the status"))?;
			if !status.is_finished() {
				return Err(tg::error!("expected the process to be finished"));
			}
			let output = server.get_process(&id).await?;
			let output = tg::process::wait::Output {
				error: output.error,
				exit: output.exit,
				output: output.output,
				status: output.status,
			};
			Ok(Some(output))
		})
	}
}

impl Server {
	pub(crate) async fn handle_post_process_wait_request<H>(
		handle: &H,
		request: http::Request<Incoming>,
		id: &str,
	) -> tg::Result<http::Response<Outgoing>>
	where
		H: tg::Handle,
	{
		// Parse the ID.
		let id = id.parse::<tg::process::Id>()?;

		// Get the accept header.
		let accept: Option<mime::Mime> = request.parse_header(http::header::ACCEPT).transpose()?;

		// Get the future.
		let future = handle.wait_process_future(&id).await?;

		// Create the stream.
		let stream = stream::once(future).filter_map(|result| async move {
			match result {
				Ok(Some(value)) => Some(Ok(tg::process::wait::Event::Output(value))),
				Ok(None) => None,
				Err(error) => Some(Err(error)),
			}
		});

		// Stop the stream when the server stops.
		let stop = request.extensions().get::<Stop>().cloned().unwrap();
		let stop = async move { stop.wait().await };
		let stream = stream.take_until(stop);

		// Create the body.
		let (content_type, body) = match accept
			.as_ref()
			.map(|accept| (accept.type_(), accept.subtype()))
		{
			Some((mime::TEXT, mime::EVENT_STREAM)) => {
				let content_type = mime::TEXT_EVENT_STREAM;
				let stream = stream.map(|result| match result {
					Ok(event) => event.try_into(),
					Err(error) => error.try_into(),
				});
				(Some(content_type), Outgoing::sse(stream))
			},

			_ => {
				return Err(tg::error!(?accept, "invalid accept header"));
			},
		};

		// Create the response.
		let mut response = http::Response::builder();
		if let Some(content_type) = content_type {
			response = response.header(http::header::CONTENT_TYPE, content_type.to_string());
		}
		let response = response.body(body).unwrap();

		Ok(response)
	}
}
