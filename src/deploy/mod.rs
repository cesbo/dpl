#![allow(dead_code)]

mod artifacts;
mod env;
mod error;
mod guard;
mod state;
mod unit;

use std::io::Read;

use env::{
    EnvError,
    EnvList,
};
pub use error::DeployError;
pub use guard::acquire;
use state::DeployState;
use unit::app::AppUnit;
pub use unit::{
    UnitConfig,
    db::{
        DbConfig,
        DbServerConfig,
        DbServerEngine,
        DbServerUnit,
        DbUnit,
    },
    list_units,
};

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
    unit.validate_references(ctx)?;

    let unit_dir = ctx.base().join(name);
    let (_guard, state) = acquire(&unit_dir)?;

    match unit {
        UnitConfig::App(config) => {
            let app = AppUnit::new(ctx, name, config);
            app.deploy(state, archive)
        }
        UnitConfig::Db(_) | UnitConfig::DbServer(_) | UnitConfig::Domain(_) => {
            Err(DeployError::UnitNotAllowed)
        }
    }
}
