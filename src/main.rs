mod archive;
mod artifacts;
mod cmd;
mod config;
mod context;
mod deploy;
mod log;
mod podman;
mod reference;
mod secret;
mod serve;
mod spinner;
mod state;
mod timers;

use std::{
    env,
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

    /// Start a unit's container (internal: called by `dpl serve`)
    #[command(hide = true)]
    Start {
        /// Unit name
        name: config::UnitName,
    },

    /// Remove a unit's active deployment from service
    Undeploy {
        /// Unit name
        name: config::UnitName,
    },

    /// Run one of a unit's timers (the timer service's ExecStart)
    Timer {
        /// Unit name
        name: config::UnitName,
        /// Timer name
        timer: String,
    },

    /// Run the in-process serve loop
    Serve,

    /// Stop serve and tear down all supervised containers (keeps deploy state)
    Down,

    /// Manage encrypted runtime secrets
    Secret(cmd::secret::Args),
}

fn main() -> ExitCode {
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

    let base = base_dir()?;
    let ctx = MainContext::load(&base).context("load main context")?;

    match cli.command {
        Command::Check { name } => cmd::unit::check(&ctx, &name),
        Command::Db(args) => cmd::db::run(&ctx, args),
        Command::Deploy { name, path } => cmd::unit::deploy(&ctx, &name, path.as_deref()),
        Command::Inspect { name } => cmd::unit::inspect(&ctx, &name),
        Command::Start { name } => cmd::unit::start(&ctx, &name),
        Command::Undeploy { name } => cmd::unit::undeploy(&ctx, &name),
        Command::Timer { name, timer } => cmd::unit::timer(&ctx, &name, &timer),
        Command::Serve => cmd::serve::run(&ctx),
        Command::Down => cmd::down::run(&ctx),
        Command::Secret(args) => cmd::secret::run(&ctx, args),
    }
}

fn base_dir() -> Result<PathBuf> {
    match env::var_os("DPL_BASE") {
        Some(value) if value.is_empty() => anyhow::bail!("DPL_BASE must not be empty"),
        Some(value) => {
            let base = PathBuf::from(value);
            std::path::absolute(&base)
                .with_context(|| format!("resolve DPL_BASE '{}'", base.display()))
        }
        None => Ok(PathBuf::from("/opt/dpl")),
    }
}
