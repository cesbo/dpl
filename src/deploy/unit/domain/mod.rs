mod artifacts;
mod model;

use std::{
    fs,
    path::PathBuf,
};

use self::artifacts::ArtifactsContext;
pub use self::model::DomainConfig;
use crate::{
    MainContext,
    deploy::{
        DeployError,
        state::DeployState,
    },
    error::format_error_chain,
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

        if let Err(err) = self.write_artifacts(version) {
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

    fn write_artifacts(&self, version: u32) -> Result<(), DeployError> {
        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));
        fs::create_dir_all(&deploy_dir).map_err(|source| DeployError::UnitError {
            info: "failed to create deploy directory".to_string(),
            source,
        })?;

        let artifacts = ArtifactsContext {
            ctx: self.ctx,
            name: &self.name,
            config: &self.config,
        };
        artifacts.save(&deploy_dir)?;

        Ok(())
    }
}
