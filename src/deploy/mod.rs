#![allow(dead_code)]

mod artifacts;
mod env;
mod error;
mod guard;
mod state;
mod unit;

use std::{
    fs,
    io::Read,
};

use env::{
    EnvError,
    EnvList,
};
pub use error::DeployError;
use state::DeployState;
pub use unit::UnitConfig;
use unit::app::AppUnit;

use crate::{
    MainContext,
    log::DeployLog,
};

pub fn deploy_unit<R: Read>(
    ctx: &MainContext,
    name: &str,
    archive: R,
) -> Result<(DeployState, DeployLog), DeployError> {
    let unit = UnitConfig::load(ctx, name)?;

    match unit {
        UnitConfig::App(config) => {
            let app = AppUnit::new(ctx, name, config);
            app.deploy(archive)
        }
        UnitConfig::Domain(_) => Err(DeployError::UnitNotAllowed),
    }
}

pub fn unit_state(ctx: &MainContext, name: &str) -> Result<DeployState, DeployError> {
    let unit_dir = ctx.base().join(name);

    let config_path = unit_dir.join("config.yaml");
    if fs::metadata(&config_path).is_err() {
        return Err(DeployError::UnitNotFound);
    }

    let state = DeployState::load(&unit_dir)?;
    Ok(state)
}
