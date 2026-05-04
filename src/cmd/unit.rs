use std::{
    error::Error,
    path::Path,
};

use clap::Subcommand;

use crate::deploy::UnitConfig;

#[derive(clap::Args)]
pub struct Args {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Validate a unit's config and secret references
    Check {
        /// Unit name
        name: String,
    },
}

pub fn run(args: Args, config_path: &Path) -> Result<(), Box<dyn Error>> {
    crate::load_main_config(config_path)?;

    match args.cmd {
        Cmd::Check { name } => check(&name),
    }
}

fn check(name: &str) -> Result<(), Box<dyn Error>> {
    let base = crate::config().base.as_path();
    UnitConfig::load(base, name)?;

    println!("ok");

    Ok(())
}
