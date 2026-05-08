mod artifacts;
mod model;

use std::path::PathBuf;

use tokio::fs;
use tracing::{
    error,
    info,
};

use self::artifacts::ArtifactsContext;
pub use self::model::DomainConfig;
use crate::{
    MainContext,
    deploy::{
        DeployError,
        state::{
            DeployState,
            DeployStatus,
        },
    },
    error::format_error_chain,
};

#[derive(Debug)]
pub struct DomainUnit {
    pub ctx: MainContext,
    pub name: String,
    pub unit_dir: PathBuf,
    pub config: DomainConfig,
}

impl DomainUnit {
    pub fn new(ctx: MainContext, name: impl Into<String>, config: DomainConfig) -> Self {
        let name = name.into();
        let unit_dir = ctx.base().join(&name);

        Self {
            ctx,
            name,
            unit_dir,
            config,
        }
    }

    pub async fn deploy(self) -> Result<DeployState, DeployError> {
        let mut state = DeployState::load(&self.unit_dir)?;
        if state.latest_build.status == DeployStatus::Building {
            return Err(DeployError::UnitBusy);
        }

        let version = state.bump_version()?;
        state.save(&self.unit_dir)?;

        info!(unit = %self.name, %version, "domain deploy started");

        if let Err(err) = self.render(version).await {
            let chain = format_error_chain(&err);
            error!(unit = %self.name, error = %chain, "render nginx config");
            state.set_error(format!("render nginx config failed: {chain}"));
            let _ = state.save(&self.unit_dir);
            return Err(err);
        }

        state.active_version = Some(version);
        state.set_ready();
        state.save(&self.unit_dir)?;

        info!(unit = %self.name, %version, "domain deploy completed");

        Ok(state)
    }

    async fn render(&self, version: u32) -> Result<(), DeployError> {
        let deploy_dir = self.unit_dir.join(format!("deploy_{version}"));
        fs::create_dir_all(&deploy_dir)
            .await
            .map_err(|source| DeployError::UnitError {
                info: "failed to create deploy directory".to_string(),
                source,
            })?;

        let artifacts = ArtifactsContext {
            ctx: &self.ctx,
            name: &self.name,
            config: &self.config,
            version,
        };
        artifacts.save(&deploy_dir).await?;

        Ok(())
    }
}
