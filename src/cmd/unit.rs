use std::{
    error::Error,
    fs,
    io,
    path::{
        Path,
        PathBuf,
    },
};

use clap::Subcommand;

use crate::{
    MainContext,
    deploy::{
        DeployState,
        UnitConfig,
        unit::app::AppUnit,
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

pub fn run(ctx: &MainContext, args: Args) -> Result<(), Box<dyn Error>> {
    match args.cmd {
        Cmd::Check { name } => check(ctx, &name),
        Cmd::Deploy { name, path } => deploy(ctx, &name, path.as_deref()),
        Cmd::State { name } => state(ctx, &name),
    }
}

fn check(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    let unit = UnitConfig::load(ctx, name)?;
    unit.validate_references(ctx)?;
    println!("ok");
    Ok(())
}

fn deploy(ctx: &MainContext, name: &str, path: Option<&Path>) -> Result<(), Box<dyn Error>> {
    let unit = UnitConfig::load(ctx, name)?;
    unit.validate_references(ctx)?;

    let UnitConfig::App(app_config) = unit else {
        return Err(format!("deploy unit '{name}': not allowed").into());
    };

    let unit_dir = ctx.base().join(name);
    let (_guard, state) = DeployState::acquire(&unit_dir)?;

    let app = AppUnit::new(ctx, name, app_config);

    let (final_state, log) = match path {
        Some(path) => {
            let file = fs::File::open(path)?;
            app.deploy(state, file)?
        }
        None => {
            let stdin = io::stdin().lock();
            app.deploy(state, stdin)?
        }
    };

    let elapsed = fmt_elapsed(log.elapsed());
    let version = final_state.latest_build.version;

    if let Some(err) = &final_state.latest_build.error {
        println!("deploy failed (version {version}) after {elapsed}: {err}");
        return Err(format!("deploy failed: {err}").into());
    }

    println!("deploy ok (version {version}) in {elapsed}");
    Ok(())
}

fn state(ctx: &MainContext, name: &str) -> Result<(), Box<dyn Error>> {
    let _unit = UnitConfig::load(ctx, name)?;
    let unit_dir = ctx.base().join(name);
    let state = DeployState::load(&unit_dir)?;
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
