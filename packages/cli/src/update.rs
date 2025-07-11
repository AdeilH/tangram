use crate::Cli;
use futures::TryStreamExt as _;
use std::path::PathBuf;
use tangram_client::{self as tg, prelude::*};

/// Update a package's lockfile.
#[derive(Clone, Debug, clap::Args)]
#[group(skip)]
pub struct Args {
	#[arg(index = 1, default_value = ".")]
	pub path: PathBuf,

	#[arg(short, long, num_args = 1.., action = clap::ArgAction::Append)]
	pub patterns: Option<Vec<tg::tag::Pattern>>,
}

impl Cli {
	pub async fn command_update(&mut self, args: Args) -> tg::Result<()> {
		let handle = self.handle().await?;
		let updates = args
			.patterns
			.unwrap_or_else(|| vec![tg::tag::Pattern::wildcard()]);

		// Get the absolute path.
		let path = std::path::absolute(&args.path)
			.map_err(|source| tg::error!(!source, "failed to get the absolute path"))?;

		// Remove an existing lockfile.
		tokio::fs::remove_file(path.clone().join(tg::package::LOCKFILE_FILE_NAME))
			.await
			.ok();

		// Check in the package.
		let arg = tg::checkin::Arg {
			destructive: false,
			deterministic: false,
			ignore: true,
			locked: false,
			lockfile: true,
			path,
			updates,
		};
		let stream = handle.checkin(arg).await?;
		stream.map_ok(|_| ()).try_collect::<()>().await?;

		Ok(())
	}
}
