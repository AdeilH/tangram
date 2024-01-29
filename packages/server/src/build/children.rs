use crate::{database::Json, params, Server};
use futures::{
	future,
	stream::{self, BoxStream},
	FutureExt, StreamExt, TryStreamExt,
};
use http_body_util::{BodyExt, StreamBody};
use num::ToPrimitive;
use std::sync::Arc;
use tangram_client as tg;
use tangram_error::{error, Error, Result, WrapErr};
use tangram_util::http::{empty, not_found, Incoming, Outgoing};
use tokio_stream::wrappers::WatchStream;

impl Server {
	pub async fn try_get_build_children(
		&self,
		id: &tg::build::Id,
		arg: tg::build::children::GetArg,
		stop: Option<tokio::sync::watch::Receiver<bool>>,
	) -> Result<Option<BoxStream<'static, Result<tg::build::children::Chunk>>>> {
		if let Some(children) = self
			.try_get_build_children_local(id, arg.clone(), stop.clone())
			.await?
		{
			Ok(Some(children))
		} else if let Some(children) = self
			.try_get_build_children_remote(id, arg.clone(), stop.clone())
			.await?
		{
			Ok(Some(children))
		} else {
			Ok(None)
		}
	}

	#[allow(clippy::too_many_lines)]
	async fn try_get_build_children_local(
		&self,
		id: &tg::build::Id,
		arg: tg::build::children::GetArg,
		stop: Option<tokio::sync::watch::Receiver<bool>>,
	) -> Result<Option<BoxStream<'static, Result<tg::build::children::Chunk>>>> {
		// Verify the build is local.
		if !self.get_build_exists_local(id).await? {
			return Ok(None);
		}

		// Create the event stream.
		let context = self.inner.build_context.read().unwrap().get(id).cloned();
		let children = context
			.as_ref()
			.map_or_else(
				|| stream::empty().left_stream(),
				|context| {
					WatchStream::from_changes(context.children.as_ref().unwrap().subscribe())
						.right_stream()
				},
			)
			.chain(stream::pending());
		let finished = {
			let server = self.clone();
			let id = id.clone();
			async move {
				let arg = tg::build::status::GetArg::default();
				server
					.try_get_build_status_local(&id, arg, None)
					.await?
					.wrap_err("Expected the build to exist.")?
					.try_filter_map(|status| {
						future::ready(Ok(if status == tg::build::Status::Finished {
							Some(())
						} else {
							None
						}))
					})
					.try_next()
					.await
			}
		};
		let timeout = arg.timeout.map_or_else(
			|| future::pending().left_future(),
			|timeout| tokio::time::sleep(timeout).right_future(),
		);
		let stop = stop.map_or_else(
			|| future::pending().left_future(),
			|mut stop| async move { stop.wait_for(|stop| *stop).map(|_| ()).await }.right_future(),
		);
		let events = stream::once(future::ready(()))
			.chain(
				children
					.take_until(finished)
					.chain(stream::once(future::ready(())))
					.take_until(timeout)
					.take_until(stop),
			)
			.boxed();

		// Get the position.
		let position = if let Some(position) = arg.position {
			position
		} else {
			self.try_get_build_children_local_current_position(id)
				.await?
		};

		// Get the length.
		let length = arg.length;

		// Get the size.
		let size = arg.size.unwrap_or(10);

		// Create the stream.
		struct State {
			position: u64,
			read: u64,
		}
		let state = State { position, read: 0 };
		let state = Arc::new(tokio::sync::Mutex::new(state));
		let stream = stream::try_unfold(
			(self.clone(), id.clone(), events, state),
			move |(server, id, mut events, state)| async move {
				let Some(()) = events.next().await else {
					return Ok(None);
				};

				// Create the stream.
				let stream = stream::try_unfold(
					(server.clone(), id.clone(), state.clone(), false),
					move |(server, id, state, end)| async move {
						if end {
							return Ok(None);
						}

						// Lock the state.
						let mut state_ = state.lock().await;

						// Determine the size.
						let size = match length {
							None => size,
							Some(length) => size.min(length - state_.read),
						};

						// Read the chunk.
						let chunk = server
							.try_get_build_children_local_inner(&id, state_.position, size)
							.await?;
						let read = chunk.data.len().to_u64().unwrap();

						// Update the state.
						state_.position += read;
						state_.read += read;

						drop(state_);

						// If the chunk is empty, then only return it if the build is finished and the position is at the end.
						if chunk.data.is_empty() {
							let end = server
								.try_get_build_children_local_end(&id, chunk.position)
								.await?;
							if end {
								return Ok::<_, Error>(Some((chunk, (server, id, state, true))));
							}
							return Ok(None);
						}

						Ok::<_, Error>(Some((chunk, (server, id, state, false))))
					},
				);

				Ok::<_, Error>(Some((stream, (server, id, events, state))))
			},
		)
		.try_flatten()
		.boxed();

		Ok(Some(stream))
	}

	async fn try_get_build_children_local_current_position(
		&self,
		id: &tg::build::Id,
	) -> Result<u64> {
		let db = self.inner.database.get().await?;
		let statement = "
			select json_array_length(state->'children')
			from builds
			where id = ?1;
		";
		let id = id.to_string();
		let params = params![id];
		let mut statement = db
			.prepare_cached(statement)
			.wrap_err("Failed to prepare the statement.")?;
		let mut rows = statement
			.query(params)
			.wrap_err("Failed to execute the statement.")?;
		let row = rows
			.next()
			.wrap_err("Failed to get the row.")?
			.wrap_err("Expected a row.")?;
		let count = row
			.get::<_, u64>(0)
			.wrap_err("Failed to deseriaize the column.")?;
		Ok(count)
	}

	async fn try_get_build_children_local_end(
		&self,
		id: &tg::build::Id,
		position: u64,
	) -> Result<bool> {
		let db = self.inner.database.get().await?;
		let statement = "
			select state->>'status' = 'finished' and ?1 = json_array_length(state->'children') as end
			from builds
			where id = ?2;
		";
		let id = id.to_string();
		let params = params![position, id];
		let mut statement = db
			.prepare_cached(statement)
			.wrap_err("Failed to prepare the statement.")?;
		let mut rows = statement
			.query(params)
			.wrap_err("Failed to execute the statement.")?;
		let row = rows
			.next()
			.wrap_err("Failed to get the row.")?
			.wrap_err("Expected a row.")?;
		let end = row
			.get::<_, bool>(0)
			.wrap_err("Failed to deseriaize the column.")?;
		Ok(end)
	}

	async fn try_get_build_children_local_inner(
		&self,
		id: &tg::build::Id,
		position: u64,
		length: u64,
	) -> Result<tg::build::children::Chunk> {
		let db = self.inner.database.get().await?;
		let statement = "
			select
				(
					select coalesce(json_group_array(value), '[]')
					from (
						select value
						from json_each(builds.state->'children')
						limit ?1
						offset ?2
					)
				) as children
			from builds
			where id = ?3;
		";
		let id = id.to_string();
		let params = params![length, position, id];
		let mut statement = db
			.prepare_cached(statement)
			.wrap_err("Failed to prepare the statement.")?;
		let mut rows = statement
			.query(params)
			.wrap_err("Failed to execute the statement.")?;
		let row = rows
			.next()
			.wrap_err("Failed to get the row.")?
			.wrap_err("Expected a row.")?;
		let children = row
			.get::<_, Json<Vec<tg::build::Id>>>(0)
			.wrap_err("Failed to deseriaize the column.")?
			.0;
		let chunk = tg::build::children::Chunk {
			position,
			data: children,
		};
		Ok(chunk)
	}

	async fn try_get_build_children_remote(
		&self,
		id: &tg::build::Id,
		arg: tg::build::children::GetArg,
		stop: Option<tokio::sync::watch::Receiver<bool>>,
	) -> Result<Option<BoxStream<'static, Result<tg::build::children::Chunk>>>> {
		let Some(remote) = self.inner.remote.as_ref() else {
			return Ok(None);
		};
		let Some(stream) = remote.try_get_build_children(id, arg).await? else {
			return Ok(None);
		};
		let stop = stop.map_or_else(
			|| future::pending().boxed(),
			|mut stop| async move { stop.wait_for(|s| *s).map(|_| ()).await }.boxed(),
		);
		let stream = stream.take_until(stop).boxed();
		Ok(Some(stream))
	}

	pub async fn add_build_child(
		&self,
		user: Option<&tg::User>,
		build_id: &tg::build::Id,
		child_id: &tg::build::Id,
	) -> Result<()> {
		if self
			.try_add_build_child_local(user, build_id, child_id)
			.await?
		{
			return Ok(());
		}
		if self
			.try_add_build_child_remote(user, build_id, child_id)
			.await?
		{
			return Ok(());
		}
		Err(error!("Failed to get the build."))
	}

	async fn try_add_build_child_local(
		&self,
		_user: Option<&tg::User>,
		build_id: &tg::build::Id,
		child_id: &tg::build::Id,
	) -> Result<bool> {
		// Verify the build is local.
		if !self.get_build_exists_local(build_id).await? {
			return Ok(false);
		}

		// Add the child to the build in the database.
		{
			let db = self.inner.database.get().await?;
			let statement = "
				update builds
				set state = json_set(state, '$.children[#]', ?1)
				where id = ?2;
			";
			let mut statement = db
				.prepare_cached(statement)
				.wrap_err("Failed to prepare the query.")?;
			statement
				.execute([child_id.to_string(), build_id.to_string()])
				.wrap_err("Failed to execute the query.")?;
		}

		// Notify subscribers that a child has been added.
		if let Some(children) = self
			.inner
			.build_context
			.read()
			.unwrap()
			.get(build_id)
			.unwrap()
			.children
			.as_ref()
		{
			children.send_replace(());
		}

		Ok(true)
	}

	async fn try_add_build_child_remote(
		&self,
		user: Option<&tg::User>,
		build_id: &tg::build::Id,
		child_id: &tg::build::Id,
	) -> Result<bool> {
		let Some(remote) = self.inner.remote.as_ref() else {
			return Ok(false);
		};
		tg::Build::with_id(child_id.clone())
			.push(user, self, remote.as_ref())
			.await?;
		remote.add_build_child(user, build_id, child_id).await?;
		Ok(true)
	}
}

impl Server {
	pub async fn handle_get_build_children_request(
		&self,
		request: http::Request<Incoming>,
	) -> Result<hyper::Response<Outgoing>> {
		// Get the path params.
		let path_components: Vec<&str> = request.uri().path().split('/').skip(1).collect();
		let ["builds", id, "children"] = path_components.as_slice() else {
			return Err(error!("Unexpected path."));
		};
		let id = id.parse().wrap_err("Failed to parse the ID.")?;

		// Get the search params.
		let arg = request
			.uri()
			.query()
			.map(serde_urlencoded::from_str)
			.transpose()
			.wrap_err("Failed to deserialize the search params.")?
			.unwrap_or_default();

		// Get the accept header.
		let accept = request
			.headers()
			.get(http::header::ACCEPT)
			.map(|accept| {
				let accept = accept.to_str().wrap_err("Invalid content type.")?;
				let accept = accept
					.parse::<mime::Mime>()
					.wrap_err("Invalid content type.")?;
				Ok::<_, Error>(accept)
			})
			.transpose()?;
		let Some(accept) = accept else {
			return Err(error!("The accept header must be set."));
		};

		// Attempt to get the children.
		let stop = request.extensions().get().cloned();
		let Some(children) = self.try_get_build_children(&id, arg, stop).await? else {
			return Ok(not_found());
		};

		// Choose the content type.
		let content_type = match (accept.type_(), accept.subtype()) {
			(mime::TEXT, mime::EVENT_STREAM) => mime::TEXT_EVENT_STREAM,
			_ => return Err(error!("Invalid accept header.")),
		};

		// Create the body.
		let body = children
			.map_ok(|chunk| {
				let data = serde_json::to_string(&chunk).unwrap();
				let event = tangram_util::sse::Event::with_data(data);
				hyper::body::Frame::data(event.to_string().into())
			})
			.map_err(Into::into);
		let body = Outgoing::new(StreamBody::new(body));

		// Create the response.
		let response = http::Response::builder()
			.status(http::StatusCode::OK)
			.header(http::header::CONTENT_TYPE, content_type.to_string())
			.body(body)
			.unwrap();

		Ok(response)
	}

	pub async fn handle_add_build_child_request(
		&self,
		request: http::Request<Incoming>,
	) -> Result<hyper::Response<Outgoing>> {
		// Get the path params.
		let path_components: Vec<&str> = request.uri().path().split('/').skip(1).collect();
		let ["builds", id, "children"] = path_components.as_slice() else {
			return Err(error!("Unexpected path."));
		};
		let build_id: tg::build::Id = id.parse().wrap_err("Failed to parse the ID.")?;

		// Get the user.
		let user = self.try_get_user_from_request(&request).await?;

		// Read the body.
		let bytes = request
			.into_body()
			.collect()
			.await
			.wrap_err("Failed to read the body.")?
			.to_bytes();
		let child_id =
			serde_json::from_slice(&bytes).wrap_err("Failed to deserialize the body.")?;

		// Add the build child.
		self.add_build_child(user.as_ref(), &build_id, &child_id)
			.await?;

		// Create the response.
		let response = http::Response::builder()
			.status(http::StatusCode::OK)
			.body(empty())
			.unwrap();

		Ok(response)
	}
}
