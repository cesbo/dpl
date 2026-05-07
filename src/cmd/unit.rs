use std::error::Error;

use clap::Subcommand;

use crate::{
    MainContext,
    deploy::UnitConfig,
};

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

pub fn run(ctx: &MainContext, args: Args) -> Result<(), Box<dyn Error>> {
    match args.cmd {
        Cmd::Check { name } => check(ctx, &name),
    }
}

fn check(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    UnitConfig::load(ctx.base(), name)?;

    println!("ok");

    Ok(())
}
