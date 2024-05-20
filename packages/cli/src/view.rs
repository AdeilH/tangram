use crate::{tui, Cli};
use tangram_client as tg;

/// View a build, object, package, or value.
#[derive(Clone, Debug, clap::Args)]
#[group(skip)]
#[command(group(clap::ArgGroup::new("arg_group").args(&["build", "object", "package", "arg"]).required(true)))]
#[group(skip)]
pub struct Args {
	/// The build to view.
	#[arg(short, long, conflicts_with_all = ["object", "package", "arg"])]
	pub build: Option<tg::build::Id>,

	/// The object to view.
	#[arg(short, long, conflicts_with_all = ["build", "package", "arg"])]
	pub object: Option<tg::object::Id>,

	/// The package to view.
	#[arg(short, long, conflicts_with_all = ["build", "object", "arg"])]
	pub package: Option<tg::Dependency>,

	/// The build, package, or object to view.
	pub arg: Option<Arg>,
}

#[derive(Clone, Debug)]
pub enum Arg {
	Build(tg::build::Id),
	Object(tg::object::Id),
	Package(tg::Dependency),
}

impl std::str::FromStr for Arg {
	type Err = tg::Error;

	fn from_str(s: &str) -> tg::Result<Self, Self::Err> {
		if let Ok(build) = s.parse() {
			return Ok(Arg::Build(build));
		}
		if let Ok(object) = s.parse() {
			return Ok(Arg::Object(object));
		}
		if let Ok(dependency) = s.parse() {
			return Ok(Arg::Package(dependency));
		}
		Err(tg::error!(%s, "expected a build or object"))
	}
}

impl Cli {
	pub async fn command_view(&self, args: Args) -> tg::Result<()> {
		let arg = if let Some(arg) = args.build {
			Arg::Build(arg)
		} else if let Some(arg) = args.object {
			Arg::Object(arg)
		} else if let Some(arg) = args.package {
			Arg::Package(arg)
		} else if let Some(arg) = args.arg {
			arg
		} else {
			return Err(tg::error!("expected an object to view."));
		};

		// Get the item.
		let item = match arg {
			Arg::Build(build) => tui::Item::Build(tg::Build::with_id(build)),
			Arg::Object(object) => tui::Item::Value {
				value: tg::Object::with_id(object).into(),
				name: None,
			},
			Arg::Package(dependency) => {
				let (artifact, lock) = tg::package::get_with_lock(&self.handle, &dependency)
					.await
					.map_err(|source| tg::error!(!source, "failed to get the package and lock"))?;
				tui::Item::Package {
					dependency,
					artifact: Some(artifact),
					lock,
				}
			},
		};

		// Start the TUI.
		let tui = tui::Tui::start(&self.handle, item).await?;

		// Wait for the TUI to finish.
		tui.wait().await?;

		Ok(())
	}
}
