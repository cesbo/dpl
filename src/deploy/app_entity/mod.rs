mod error;
mod model;
mod port;
mod templates;

use std::path::PathBuf;

pub use error::AppEntityError;
use model::AppConfig;
use tokio::fs;

use crate::{
    deploy::{
        DeployError,
        EntityType,
        get_entity_version,
        load_entity_config,
    },
    error::ConfigError,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppEntity {
    pub dir: PathBuf,
    pub name: String,
    pub config: AppConfig,
    pub version: u32,
    pub port: u16,
}

impl AppEntity {
    pub async fn load(name: &str, dir: PathBuf) -> Result<Self, ConfigError> {
        let name = name.to_owned();
        let config: AppConfig = load_entity_config(&dir).await?;

        Ok(AppEntity {
            dir,
            name,
            config,
            version: 0,
            port: 0,
        })
    }

    pub async fn prepare(&mut self) -> Result<Self, DeployError> {
        let status = crate::deploy::read_deploy_status(&self.dir)
            .await
            .map_err(|err| DeployError::StatusError(err))?;

        if status == crate::deploy::DeployStatus::Building {
            return Err(DeployError::EntityBusy);
        }

        self.version = crate::deploy::reserve_entity_version(&self.dir)
            .await
            .map_err(|err| DeployError::VersionError(err))?;

        let deploy_dir_name = format!("deploy_{}", self.version);
        let deploy_dir = self.dir.join(deploy_dir_name);

        fs::create_dir(&deploy_dir)
            .await
            .map_err(|err| DeployError::DeployDirectoryError(err))?;

        self.port = port::get_port(&self.dir)
            .await
            .map_err(|err| AppEntityError::PortError(err))?;

        // TODO: continue here...

        // artifacts::write_artifacts(self, deploy_dir).await

        unimplemented!()
    }
}
