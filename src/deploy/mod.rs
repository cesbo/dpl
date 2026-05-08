#![allow(dead_code)]

mod artifacts;
mod env;
mod error;
mod guard;
mod state;
mod unit;

use std::path::Path;

use env::{
    EnvError,
    EnvList,
};
pub use error::DeployError;
use state::DeployState;
use tokio::{
    fs,
    io::AsyncRead,
    task::JoinHandle,
};
pub use unit::UnitConfig;
use unit::app::AppUnit;

use crate::{
    MainContext,
    log::DeployLog,
};

pub async fn deploy_unit<R>(
    base: &Path,
    name: &str,
    archive: R,
) -> Result<(DeployState, DeployLog, JoinHandle<()>), DeployError>
where
    R: AsyncRead + Unpin + Send,
{
    let ctx = MainContext::load(base)?;
    let unit = UnitConfig::load(&ctx, name)?;

    match unit {
        UnitConfig::App(config) => {
            let app = AppUnit::new(ctx, name, config);
            let (state, deploy_dir) = app.prepare_deploy(archive).await?;
            let version = state.latest_build.version;
            let log_path = deploy_dir.join("log").join("build.log");

            let log = match DeployLog::open(&log_path, &app.name, version) {
                Ok(log) => log,
                Err(source) => {
                    let mut state = state;
                    state.set_error(format!("open deploy log: {source}"));
                    let _ = state.save(&app.unit_dir);
                    return Err(DeployError::UnitError {
                        info: "open deploy log".to_string(),
                        source,
                    });
                }
            };

            let handle = app.start_worker(deploy_dir, state.clone(), log.clone());
            Ok((state, log, handle))
        }
        UnitConfig::Domain(_) => Err(DeployError::UnitNotAllowed),
    }
}

pub async fn unit_state(base: &Path, name: &str) -> Result<DeployState, DeployError> {
    let ctx = MainContext::load(base)?;
    let unit_dir = ctx.base().join(name);

    let config_path = unit_dir.join("config.yaml");
    if fs::metadata(&config_path).await.is_err() {
        return Err(DeployError::UnitNotFound);
    }

    let state = DeployState::load(&unit_dir)?;
    Ok(state)
}
