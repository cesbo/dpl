use std::{
    error::Error,
    path::{
        Path,
        PathBuf,
    },
};

use clap::Subcommand;
use tokio::{
    fs,
    io,
};

use crate::{
    MainContext,
    deploy::{
        DeployService,
        UnitConfig,
    },
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
    /// Trigger a deploy from a tar.gz archive (stdin if path is omitted)
    Deploy {
        /// Unit name
        name: String,
        /// Path to tar.gz archive; reads from stdin if omitted
        path: Option<PathBuf>,
    },
    /// Show deploy state for a unit
    State {
        /// Unit name
        name: String,
    },
}

pub async fn run(ctx: &MainContext, args: Args) -> Result<(), Box<dyn Error>> {
    match args.cmd {
        Cmd::Check { name } => check(ctx, &name),
        Cmd::Deploy { name, path } => deploy(&name, path.as_deref()).await,
        Cmd::State { name } => state(&name).await,
    }
}

fn check(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    UnitConfig::load(ctx, name)?;
    println!("ok");
    Ok(())
}

async fn deploy(name: &str, path: Option<&Path>) -> Result<(), Box<dyn Error>> {
    let service = DeployService::global();

    let (state, handle) = match path {
        Some(path) => {
            let file = fs::File::open(path).await?;
            service.deploy(name, file).await?
        }
        None => service.deploy(name, io::stdin()).await?,
    };

    println!("started version {}", state.latest_build.version);

    let _ = handle.await;
    Ok(())
}

async fn state(name: &str) -> Result<(), Box<dyn Error>> {
    let state = DeployService::global().state(name).await?;
    let build = &state.latest_build;
    let status = format!("{:?}", build.status).to_lowercase();

    println!("version: {}", build.version);
    println!("status:  {status}");
    if let Some(active) = state.active_version {
        println!("active:  {active}");
    }
    if let Some(err) = &build.error {
        println!("error:   {err}");
    }
    Ok(())
}
