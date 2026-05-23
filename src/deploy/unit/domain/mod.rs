mod artifacts;
mod host_name;
mod model;
mod route_location;

use std::path::PathBuf;

use self::artifacts::ArtifactsContext;
pub use self::model::DomainConfig;
use crate::{
    MainContext,
    deploy::{
        DeployError,
        state::DeployState,
        unit::NGINX_VOLUME,
    },
    error::format_error_chain,
    podman::{
        ensure_volume,
        volume_mountpoint,
    },
};

#[derive(Debug)]
pub struct DomainUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: String,
    pub unit_dir: PathBuf,
    pub config: DomainConfig,
}

impl<'a> DomainUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: impl Into<String>, config: DomainConfig) -> Self {
        let name = name.into();
        let unit_dir = ctx.base().join(&name);

        Self {
            ctx,
            name,
            unit_dir,
            config,
        }
    }

    pub fn deploy(self, mut state: DeployState) -> Result<DeployState, DeployError> {
        let version = state.bump_version()?;
        state.save(&self.unit_dir)?;

        if let Err(err) = self.write_config() {
            let chain = format_error_chain(&err);
            eprintln!("render nginx config failed for {}: {chain}", self.name);
            state.set_error(format!("render nginx config failed: {chain}"));
            let _ = state.save(&self.unit_dir);
            return Err(err);
        }

        state.active_version = Some(version);
        state.set_ready();
        state.save(&self.unit_dir)?;

        Ok(state)
    }

    fn write_config(&self) -> Result<(), DeployError> {
        ensure_volume(NGINX_VOLUME).map_err(|source| DeployError::UnitError {
            info: format!("failed to get nginx volume '{NGINX_VOLUME}'"),
            source,
        })?;

        let conf_dir = volume_mountpoint(NGINX_VOLUME)
            .map_err(|source| DeployError::UnitError {
                info: format!("failed to resolve nginx volume '{NGINX_VOLUME}' mountpoint"),
                source,
            })?
            .join("nginx")
            .join("conf.d");

        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            name: &self.name,
            config: &self.config,
        };
        artifacts.save(&conf_dir)?;

        Ok(())
    }
}
