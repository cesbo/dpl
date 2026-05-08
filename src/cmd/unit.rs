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
        self,
        UnitConfig,
    },
    log::fmt_elapsed,
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
        Cmd::Deploy { name, path } => deploy(ctx, &name, path.as_deref()).await,
        Cmd::State { name } => state(ctx, &name).await,
    }
}

fn check(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    UnitConfig::load(ctx, name)?;
    println!("ok");
    Ok(())
}

async fn deploy(
    ctx: &MainContext,
    name: &str,
    path: Option<&Path>,
) -> Result<(), Box<dyn Error>> {
    let (_state, log, handle) = match path {
        Some(path) => {
            let file = fs::File::open(path).await?;
            deploy::deploy_unit(ctx.base(), name, file).await?
        }
        None => deploy::deploy_unit(ctx.base(), name, io::stdin()).await?,
    };

    let _ = handle.await;

    let final_state = deploy::unit_state(ctx.base(), name).await?;
    let elapsed = fmt_elapsed(log.elapsed());
    let version = final_state.latest_build.version;

    if let Some(err) = &final_state.latest_build.error {
        println!("deploy failed (version {version}) after {elapsed}: {err}");
        return Err(format!("deploy failed: {err}").into());
    }

    println!("deploy ok (version {version}) in {elapsed}");
    Ok(())
}

async fn state(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    let state = deploy::unit_state(ctx.base(), name).await?;
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
