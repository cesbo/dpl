mod artifacts;
mod model;
mod sql;
mod systemd;

use std::{
    fs,
    path::PathBuf,
};

pub use self::model::{
    DbConfig,
    DbServerConfig,
    DbServerEngine,
};
use crate::{
    MainContext,
    deploy::{
        DeployError,
        state::DeployState,
    },
    error::format_error_chain,
};

#[derive(Debug)]
pub struct DbServerUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: String,
    pub unit_dir: PathBuf,
    pub config: DbServerConfig,
}

impl<'a> DbServerUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: impl Into<String>, config: DbServerConfig) -> Self {
        let name = name.into();
        let unit_dir = ctx.base().join(&name);

        Self {
            ctx,
            name,
            unit_dir,
            config,
        }
    }

    /// One-shot bring-up: decrypt the dpl secret, render the systemd unit
    /// (with the password inlined into `Environment=`), then install and start
    /// the service. Mirrors `DomainUnit::deploy` lifecycle. The caller holds
    /// the busy lock and provides freshly loaded state.
    pub fn init(self, mut state: DeployState) -> Result<DeployState, DeployError> {
        let version = state.bump_version()?;
        state.save(&self.unit_dir)?;

        if let Err(err) = self.bring_up(version) {
            let chain = format_error_chain(&err);
            eprintln!("db init failed for {}: {chain}", self.name);
            state.set_error(format!("db init failed: {chain}"));
            let _ = state.save(&self.unit_dir);
            return Err(err);
        }

        state.active_version = Some(version);
        state.set_ready();
        state.save(&self.unit_dir)?;

        Ok(state)
    }

    fn bring_up(&self, version: u32) -> Result<(), DeployError> {
        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));
        fs::create_dir_all(&deploy_dir).map_err(|source| DeployError::UnitError {
            info: "failed to create deploy directory".to_string(),
            source,
        })?;

        let password = self
            .ctx
            .resolve_secret(&self.config.secret)
            .map_err(|err| DeployError::UnitError {
                info: format!("decrypt secret '{}'", self.config.secret),
                source: std::io::Error::other(err),
            })?;

        let artifacts = artifacts::ArtifactsContext {
            ctx: self.ctx,
            name: &self.name,
            config: &self.config,
            version,
            password: &password,
        };
        artifacts.save(&deploy_dir)?;

        systemd::install_service(&self.name, &deploy_dir).map_err(|source| {
            DeployError::UnitError {
                info: format!("install systemd service for '{}'", self.name),
                source,
            }
        })?;

        Ok(())
    }
}
