#![allow(unused_imports)]

mod model;

use std::{
    fs,
    path::{
        Path,
        PathBuf,
    },
};

use model::AppConfig;

use crate::error::ConfigError;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppEntity {
    pub name: String,
    pub config: AppConfig,
}

impl AppEntity {
    pub fn load(base_dir: &Path, name: &str) -> Result<Self, ConfigError> {
        let entity_dir = base_dir.join(name);
        let path = entity_dir.join("config.yaml");

        let contents = match fs::read_to_string(&path) {
            Ok(v) => v,
            Err(source) => {
                return Err(ConfigError::Read { path, source });
            }
        };

        let config: AppConfig = match serde_yaml::from_str(&contents) {
            Ok(v) => v,
            Err(source) => {
                return Err(ConfigError::Parse { path, source });
            }
        };

        let name = name.to_owned();

        Ok(AppEntity { name, config })
    }
}
