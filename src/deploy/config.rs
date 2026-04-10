use std::path::Path;

use serde::de::DeserializeOwned;
use tokio::fs;

use crate::config::ConfigError;

pub async fn load_entity_config<T: DeserializeOwned>(entity_dir: &Path) -> Result<T, ConfigError> {
    let path = entity_dir.join("config.yaml");

    let content = match fs::read_to_string(&path).await {
        Ok(v) => v,
        Err(source) => {
            return Err(ConfigError::Read { path, source });
        }
    };

    serde_yaml::from_str::<T>(&content).map_err(|source| ConfigError::Parse { path, source })
}
