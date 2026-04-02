mod artifacts;
mod error;
mod model;
mod port;

use std::path::PathBuf;

pub use error::AppEntityError;
use model::AppConfig;
use tokio::fs;

use crate::{
    deploy::{
        DeployError,
        load_entity_config,
    },
    error::ConfigError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppEntity {
    pub dir: PathBuf,
    pub name: String,
    pub config: AppConfig,
}

impl AppEntity {
    pub async fn load(name: &str, dir: PathBuf) -> Result<Self, ConfigError> {
        let name = name.to_owned();
        let config: AppConfig = load_entity_config(&dir).await?;

        Ok(AppEntity { dir, name, config })
    }

    pub async fn prepare(&mut self) -> Result<Self, DeployError> {
        let status = crate::deploy::read_deploy_status(&self.dir)
            .await
            .map_err(|err| DeployError::StatusError(err))?;

        if status == crate::deploy::DeployStatus::Building {
            return Err(DeployError::EntityBusy);
        }

        let version = crate::deploy::reserve_entity_version(&self.dir)
            .await
            .map_err(|err| DeployError::VersionError(err))?;

        let deploy_dir_name = format!("deploy_{}", version);
        let deploy_dir = self.dir.join(deploy_dir_name);

        fs::create_dir(&deploy_dir)
            .await
            .map_err(|err| DeployError::DeployDirectoryError(err))?;

        let port = port::get_port(&self.dir)
            .await
            .map_err(|err| AppEntityError::PortError(err))?;

        // TODO: continue here...

        // artifacts::write_artifacts(self, deploy_dir).await

        unimplemented!()
    }
}
