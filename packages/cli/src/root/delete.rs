use crate::Cli;
use tangram_client as tg;
use tg::Handle as _;

/// Delete a root.
#[derive(Clone, Debug, clap::Args)]
#[group(skip)]
pub struct Args {
	pub name: String,
}

impl Cli {
	pub async fn command_root_delete(&self, args: Args) -> tg::Result<()> {
		self.handle.delete_root(&args.name).await?;
		Ok(())
	}
}