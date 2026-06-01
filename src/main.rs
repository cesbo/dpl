mod archive;
mod cmd;
mod config;
mod context;
mod deploy;
mod error;
mod log;
mod podman;
mod secret;
mod spinner;
mod systemd;

use std::{
    path::PathBuf,
    process::ExitCode,
};

use anyhow::{
    Context,
    Result,
};
use clap::{
    Parser,
    Subcommand,
};

pub use self::context::MainContext;
use crate::deploy::DeployError;

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
        name: config::UnitName,
    },
    /// Manage database units
    Db(cmd::db::Args),
    /// Deploy a unit (app: tar.gz archive; db: optional SQL backup to restore)
    Deploy {
        /// Unit name
        name: config::UnitName,
        /// For an app unit: tar.gz archive (required; `-` for stdin).
        /// For a db unit: SQL dump to restore (`-` for stdin; omit for
        /// provision only). Gzip is detected automatically.
        path: Option<PathBuf>,
    },
    /// Show runtime state of a unit
    Inspect {
        /// Unit name
        name: config::UnitName,
    },
    /// Start a unit's container (the systemd service's ExecStart)
    Start {
        /// Unit name
        name: config::UnitName,
    },
    /// Stop a unit's container (the systemd service's ExecStop)
    Stop {
        /// Unit name
        name: config::UnitName,
    },
    /// Manage encrypted runtime secrets
    Secret(cmd::secret::Args),
}

fn main() -> ExitCode {
    log::init();

    match run() {
        Ok(()) => ExitCode::SUCCESS,
        // A reported deploy failure already showed its summary on the console;
        // everything else gets the full anyhow report here.
        Err(err) => {
            if !matches!(
                err.downcast_ref::<DeployError>(),
                Some(DeployError::Reported)
            ) {
                eprintln!("Error: {err:?}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    let ctx = MainContext::load(&cli.base).context("load main context")?;

    match cli.command {
        Command::Check { name } => cmd::unit::check(&ctx, &name),
        Command::Db(args) => cmd::db::run(&ctx, args),
        Command::Deploy { name, path } => cmd::unit::deploy(&ctx, &name, path.as_deref()),
        Command::Inspect { name } => cmd::unit::inspect(&ctx, &name),
        Command::Start { name } => cmd::unit::start(&ctx, &name),
        Command::Stop { name } => cmd::unit::stop(&ctx, &name),
        Command::Secret(args) => cmd::secret::run(&ctx, args),
    }
}
