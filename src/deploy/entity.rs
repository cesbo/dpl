use std::{
    fmt,
    path::Path,
};

use serde::Deserialize;

use crate::{
    config::{
        ValidateConfig,
        load_config,
    },
    deploy::{
        DeployError,
        app_entity::AppEntity,
    },
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    App,
    Domain,
    Static,
    Database,
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::App => "app",
            Self::Domain => "domain",
            Self::Static => "static",
            Self::Database => "database",
        };

        f.write_str(value)
    }
}

#[derive(Deserialize)]
struct EntityMeta {
    #[serde(rename = "type")]
    pub entity_type: EntityType,
}

impl ValidateConfig for EntityMeta {}

pub enum DeployEntity {
    App(AppEntity),
}

impl DeployEntity {
    pub fn load(name: &str, entity_dir: &Path) -> Result<Self, DeployError> {
        let path = entity_dir.join("config.yaml");
        let meta: EntityMeta = load_config(&path).map_err(|err| {
            if err.is_not_found() {
                DeployError::EntityNotFound
            } else {
                DeployError::EntityConfig(err)
            }
        })?;
        match meta.entity_type {
            EntityType::App => {
                let entity = AppEntity::load(name, entity_dir)?;
                Ok(Self::App(entity))
            }
            _ => unimplemented!(),
        }
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
