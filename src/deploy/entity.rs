use std::path::Path;

use serde::Deserialize;

use super::app_entity::AppConfig;
use crate::{
    config::{
        ValidateConfig,
        load_config,
    },
    deploy::DeployError,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EntityConfig {
    App(AppConfig),
}

impl ValidateConfig for EntityConfig {
    fn validate_config(&self) -> Result<(), String> {
        match self {
            EntityConfig::App(config) => config.validate_config(),
        }
    }
}

impl EntityConfig {
    pub fn load(entity_dir: &Path) -> Result<Self, DeployError> {
        let path = entity_dir.join("config.yaml");
        let entity = load_config(&path).map_err(|err| {
            if err.is_not_found() {
                DeployError::EntityNotFound
            } else {
                DeployError::EntityConfig(err)
            }
        })?;

        Ok(entity)
    }
}

pub fn validate_name(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    if name.starts_with('-') || name.ends_with('-') {
        return false;
    }

    if name.contains("--") {
        return false;
    }

    name.as_bytes()
        .iter()
        .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
