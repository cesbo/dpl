use std::path::Path;

use serde::Deserialize;

use super::{
    app_entity::AppConfig,
    domain_entity::DomainConfig,
};
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
    Domain(DomainConfig),
}

impl ValidateConfig for EntityConfig {
    fn validate_config(&self) -> Result<(), String> {
        match self {
            EntityConfig::App(config) => config.validate_config(),
            EntityConfig::Domain(config) => config.validate_config(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ValidateConfig;

    #[test]
    fn parse_domain_entity_config() {
        let config: EntityConfig = serde_yaml::from_str(
            r#"
type: domain
proxy:
  type: cloudflare
https: proxy
routes:
  - app: backend
    resource: static
"#,
        )
        .unwrap();

        assert!(matches!(config, EntityConfig::Domain(_)));
        assert!(config.validate_config().is_ok());
    }
}
