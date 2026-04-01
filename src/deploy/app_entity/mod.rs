mod error;
mod model;
mod port;
mod templates;

use std::path::PathBuf;

pub use error::AppEntityError;
use model::AppConfig;

use crate::deploy::{
    get_entity_version,
    load_entity_config,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppEntity {
    pub dir: PathBuf,
    pub name: String,
    pub version: u32,
    pub config: AppConfig,
    pub port: u16,
}

impl AppEntity {
    pub async fn load(name: &str) -> Result<Self, AppEntityError> {
        let dir = crate::config::ENV.base_dir.join(name);
        let name = name.to_owned();
        let config = load_entity_config(&dir).await?;

        let port = port::get_port(&dir)
            .await
            .map_err(|err| AppEntityError::PortError(err))?;

        let version = get_entity_version(&dir)
            .await
            .map_err(|err| AppEntityError::VersionError(err))?;

        Ok(AppEntity {
            dir,
            name,
            version,
            port,
            config,
        })
    }
}
