use crate::{runtime, BuildPermit, Server};
use futures::{future, Future, FutureExt as _, TryFutureExt as _};
use std::{sync::Arc, time::Duration};
use tangram_client::{self as tg, handle::Ext as _};
use tangram_either::Either;
use tangram_futures::task::Task;

impl Server {
	pub(crate) async fn process_queue_task(&self) {
		loop {
			// Wait for a permit.
			let permit = self
				.process_semaphore
				.clone()
				.acquire_owned()
				.await
				.unwrap();
			let permit = BuildPermit(Either::Left(permit));

			// Try to dequeue a process locally or from one of the remotes.
			let arg = tg::process::dequeue::Arg::default();
			let futures = std::iter::once(
				self.dequeue_process(arg)
					.map_ok(|output| tg::Process::new(output.process, None, None, None))
					.boxed(),
			)
			.chain(
				self.config
					.process
					.as_ref()
					.unwrap()
					.remotes
					.iter()
					.map(|name| {
						let server = self.clone();
						let remote = name.to_owned();
						async move {
							let client = server.get_remote_client(remote).await?;
							let arg = tg::process::dequeue::Arg::default();
							let output = client.dequeue_process(arg).await?;
							let process =
								tg::Process::new(output.process, Some(name.clone()), None, None);
							Ok::<_, tg::Error>(process)
						}
						.boxed()
					}),
			);
			let process = match future::select_ok(futures).await {
				Ok((process, _)) => process,
				Err(error) => {
					tracing::error!(?error, "failed to dequeue a process");
					tokio::time::sleep(Duration::from_secs(1)).await;
					continue;
				},
			};

			// Run the process.
			self.spawn_process_task(&process, permit).await.ok();
		}
	}

	pub(crate) fn spawn_process_task(
		&self,
		process: &tg::Process,
		permit: BuildPermit,
	) -> impl Future<Output = tg::Result<()>> + Send + 'static {
		let server = self.clone();
		let process = process.clone();
		async move {
			// Attempt to start the process.
			let arg = tg::process::start::Arg {
				remote: process.remote().cloned(),
			};
			let started = server.try_start_process(process.id(), arg).await?.started;
			if !started {
				return Ok(());
			}

			// Spawn the process task.
			server.processes.spawn(
				process.id().clone(),
				Task::spawn(|_| {
					let server = server.clone();
					let process = process.clone();
					async move { server.process_task(&process, permit).await }
						.inspect_err(|error| {
							tracing::error!(?error, "the process task failed");
						})
						.map(|_| ())
				}),
			);

			// Spawn the heartbeat task.
			tokio::spawn({
				let server = server.clone();
				let process = process.clone();
				async move { server.heartbeat_task(&process).await }
					.inspect_err(|error| {
						tracing::error!(?error, "the heartbeat task failed");
					})
					.map(|_| ())
			});

			Ok(())
		}
	}

	async fn process_task(&self, process: &tg::Process, permit: BuildPermit) -> tg::Result<()> {
		// Set the process's permit.
		let permit = Arc::new(tokio::sync::Mutex::new(Some(permit)));
		self.process_permits.insert(process.id().clone(), permit);
		scopeguard::defer! {
			self.process_permits.remove(process.id());
		}

		// Run.
		let output = self.process_task_inner(process).await?;

		// Compute the status.
		let status = match (&output.output, &output.exit, &output.error) {
			(_, _, Some(_)) | (_, Some(tg::process::Exit::Signal { signal: _ }), _) => {
				tg::process::Status::Failed
			},
			(_, Some(tg::process::Exit::Code { code }), _) if *code != 0 => {
				tg::process::Status::Failed
			},
			_ => tg::process::Status::Succeeded,
		};

		// Get the output data.
		let value = match &output.output {
			Some(output) => Some(output.data(self).await?),
			// If the process succeeded but had no output, mark it as having the value "null"
			None if status == tg::process::Status::Succeeded => Some(tg::value::Data::Null),
			None => None,
		};

		// Finish the process.
		self.try_finish_process_local(process.id(), output.error, value, output.exit, status)
			.await?;

		Ok::<_, tg::Error>(())
	}

	async fn process_task_inner(&self, process: &tg::Process) -> tg::Result<runtime::Output> {
		let command = process.command(self).await?;
		let host = command.host(self).await?;
		let runtime = self
			.runtimes
			.read()
			.unwrap()
			.get(&*host)
			.ok_or_else(
				|| tg::error!(?id = process, ?host = &*host, "failed to find a runtime for the process"),
			)?
			.clone();
		Ok(runtime.run(process).await)
	}

	async fn heartbeat_task(&self, process: &tg::Process) -> tg::Result<()> {
		let interval = self.config.process.as_ref().unwrap().heartbeat_interval;
		loop {
			let arg = tg::process::heartbeat::Arg {
				remote: process.remote().cloned(),
			};
			let result = self.heartbeat_process(process.id(), arg).await;
			if let Ok(output) = result {
				if output.status.is_finished() {
					self.processes.abort(process.id());
					break;
				}
			}
			tokio::time::sleep(interval).await;
		}
		Ok(())
	}
}
