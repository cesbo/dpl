use std::fmt;

use serde::Deserialize;

use crate::deploy::{
    DeployError,
    app_entity::AppEntity,
    config::load_entity_config,
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

pub enum DeployEntity {
    App(AppEntity),
}

impl DeployEntity {
    pub async fn load(name: &str) -> Result<Self, DeployError> {
        let dir = crate::config::ENV.base_dir.join(name);
        let meta: EntityMeta = load_entity_config(&dir).await?;
        match meta.entity_type {
            EntityType::App => {
                let entity = AppEntity::load(name, dir).await?;
                Ok(Self::App(entity))
            }
            _ => unimplemented!(),
        }
    }
}
