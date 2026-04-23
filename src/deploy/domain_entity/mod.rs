mod artifacts;
mod model;

use std::path::{
    Path,
    PathBuf,
};

use artifacts::ArtifactsContext;
pub use model::DomainConfig;
use tokio::fs;
use tracing::{
    error,
    info,
};

use crate::deploy::{
    DeployError,
    state::{
        DeployState,
        DeployStatus,
    },
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainEntity {
    pub name: String,
    pub entity_dir: PathBuf,
    pub config: DomainConfig,
}

impl DomainEntity {
    pub fn new(name: &str, entity_dir: &Path, config: &DomainConfig) -> Self {
        Self {
            name: name.into(),
            entity_dir: entity_dir.into(),
            config: config.clone(),
        }
    }

    pub async fn deploy(self) -> Result<DeployState, DeployError> {
        let mut state = DeployState::load(&self.entity_dir)?;
        if state.latest_build.status == DeployStatus::Building {
            return Err(DeployError::EntityBusy);
        }

        let version = state.bump_version()?;
        state.save(&self.entity_dir)?;

        info!(entity = %self.name, %version, "domain deploy started");

        if let Err(err) = self.render(version).await {
            error!(entity = %self.name, error = %err, "render nginx config");
            state.set_error(format!("render nginx config failed: {err}"));
            let _ = state.save(&self.entity_dir);
            return Err(err);
        }

        state.active_version = Some(version);
        state.set_ready();
        state.save(&self.entity_dir)?;

        info!(entity = %self.name, %version, "domain deploy completed");

        Ok(state)
    }

    async fn render(&self, version: u32) -> Result<(), DeployError> {
        let deploy_dir = self.entity_dir.join(format!("deploy_{version}"));
        fs::create_dir_all(&deploy_dir)
            .await
            .map_err(|source| DeployError::EntityError {
                info: "failed to create deploy directory".to_string(),
                source,
            })?;

        let artifacts = ArtifactsContext {
            name: &self.name,
            config: &self.config,
            version,
        };
        artifacts.save(&deploy_dir).await?;

        Ok(())
    }
}
