mod archive;
mod cmd;
mod config;
mod context;
mod deploy;
mod error;
mod log;
mod secret;
mod systemd;
mod validate;

use std::path::PathBuf;

use clap::{
    Parser,
    Subcommand,
};
use miette::{
    Context,
    IntoDiagnostic,
    Result,
};

pub use self::context::MainContext;

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Base directory
    #[arg(long = "base", default_value = "/opt/dpl", global = true)]
    base: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Validate a unit's config and reference graph
    Check {
        /// Unit name
        name: String,
    },
    /// Manage database units
    Db(cmd::db::Args),
    /// Show runtime state of a unit
    Inspect {
        /// Unit name
        name: String,
    },
    /// Manage encrypted runtime secrets
    Secret(cmd::secret::Args),
    /// Manage units
    Unit(cmd::unit::Args),
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let ctx = MainContext::load(&cli.base)
        .into_diagnostic()
        .wrap_err("load main context")?;

    match cli.command {
        Command::Check { name } => cmd::unit::check(&ctx, &name),
        Command::Db(args) => cmd::db::run(&ctx, args),
        Command::Inspect { name } => cmd::unit::inspect(&ctx, &name),
        Command::Secret(args) => cmd::secret::run(&ctx, args),
        Command::Unit(args) => cmd::unit::run(&ctx, args),
    }
}
