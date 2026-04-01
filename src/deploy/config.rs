use std::path::Path;

use serde::de::DeserializeOwned;
use tokio::fs;

use crate::error::ConfigError;

pub async fn load_entity_config<T: DeserializeOwned>(dir: &Path) -> Result<T, ConfigError> {
    let path = dir.join("config.yaml");

    let contents = match fs::read_to_string(&path).await {
        Ok(v) => v,
        Err(source) => {
            return Err(ConfigError::Read { path, source });
        }
    };

    let config = match serde_yaml::from_str::<T>(&contents) {
        Ok(v) => v,
        Err(source) => {
            return Err(ConfigError::Parse { path, source });
        }
    };

    Ok(config)
}
