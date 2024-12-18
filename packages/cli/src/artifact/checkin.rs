use crate::Cli;
use std::path::PathBuf;
use tangram_client::{self as tg, Handle as _};

/// Check in an artifact.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, clap::Args)]
#[group(skip)]
#[allow(clippy::struct_excessive_bools)]
pub struct Args {
	/// Check in the artifact faster by allowing it to be destroyed.
	#[arg(long)]
	pub cache: bool,

	/// Check in the artifact faster by allowing it to be destroyed.
	#[arg(long)]
	pub destructive: bool,

	/// Check in the artifact determnistically.
	#[arg(long)]
	pub deterministic: bool,

	/// If false, don't parse ignore files.
	#[arg(default_value = "true", long, action = clap::ArgAction::Set)]
	pub ignore: bool,

	/// If this flag is set, lockfiles will not be updated.
	#[arg(long)]
	pub locked: bool,

	/// If false, don't write lockfiles.
	#[arg(default_value = "true", long, action = clap::ArgAction::Set)]
	pub lockfile: bool,

	/// The path to check in.
	#[arg(index = 1)]
	pub path: Option<PathBuf>,
}

impl Cli {
	pub async fn command_artifact_checkin(&self, args: Args) -> tg::Result<()> {
		let handle = self.handle().await?;

		// Get the path.
		let path = std::path::absolute(args.path.unwrap_or_default())
			.map_err(|source| tg::error!(!source, "failed to get the path"))?;

		// Check in the artifact.
		let arg = tg::artifact::checkin::Arg {
			cache: args.cache,
			destructive: args.destructive,
			deterministic: false,
			ignore: args.ignore,
			locked: args.locked,
			lockfile: args.lockfile,
			path,
		};
		let stream = handle
			.check_in_artifact(arg)
			.await
			.map_err(|source| tg::error!(!source, "failed to check in the artifact"))?;
		let output = self.render_progress_stream(stream).await?;

		// Print the artifact.
		println!("{}", output.artifact);

		Ok(())
	}
}
