mod archive;
mod cmd;
mod config;
mod context;
mod deploy;
mod error;
mod log;
mod secret;
mod validate;

use std::{
    error::Error,
    path::PathBuf,
};

use clap::{
    Parser,
    Subcommand,
};

pub use self::context::MainContext;
use self::error::exit_with_stderr;

#[derive(Parser)]
struct Cli {
    /// Base directory
    #[arg(long = "base", default_value = "/opt/dpl", global = true)]
    base: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage database units
    Db(cmd::db::Args),
    /// Manage encrypted runtime secrets
    Secret(cmd::secret::Args),
    /// Manage units
    Unit(cmd::unit::Args),
}

fn main() {
    let cli = Cli::parse();

    let result: Result<(), Box<dyn Error>> = match MainContext::load(&cli.base) {
        Ok(ctx) => match cli.command {
            Command::Db(args) => cmd::db::run(&ctx, args),
            Command::Secret(args) => cmd::secret::run(&ctx, args),
            Command::Unit(args) => cmd::unit::run(&ctx, args),
        },
        Err(err) => Err(err.into()),
    };

    if let Err(err) = result {
        exit_with_stderr(err.as_ref());
    }
}
