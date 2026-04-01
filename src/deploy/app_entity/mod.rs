mod artifacts;
mod error;
mod model;
mod port;
mod templates;

use std::path::PathBuf;

pub use error::{
    AppEntityError,
    ArtifactError,
};
use model::AppConfig;

use crate::deploy::{
    EntityType,
    get_entity_version,
    load_entity_config,
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
    pub async fn load(name: &str, dir: PathBuf) -> Result<Self, AppEntityError> {
        let name = name.to_owned();
        let config: AppConfig = load_entity_config(&dir).await?;

        if config.entity_type != EntityType::App {
            return Err(AppEntityError::InvalidEntityType(config.entity_type));
        }

        let port = port::get_port(&dir)
            .await
            .map_err(|err| AppEntityError::PortError(err))?;

        let version = get_entity_version(&dir)
            .await
            .map_err(|err| AppEntityError::VersionError(err))?;

        Ok(AppEntity {
            dir,
            name,
            config,
            version,
            port,
        })
    }

    pub async fn prepare(&self) -> Result<Self, AppEntityError> {
        let status = crate::deploy::read_deploy_status(&self.dir)
            .await
            .map_err(|err| AppEntityError::StatusError(err))?;

        if status == crate::deploy::DeployStatus::Building {
            return Err(AppEntityError::BuildInProgress);
        }

        let version = crate::deploy::reserve_entity_version(&self.dir)
            .await
            .map_err(|err| AppEntityError::VersionError(err))?;

        // TODO: continue here...

        // artifacts::write_artifacts(self, deploy_dir).await

        unimplemented!()
    }
}
