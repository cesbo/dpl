use std::path::Path;

use serde::Deserialize;

use super::{
    app::AppConfig,
    domain::DomainConfig,
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
pub enum UnitConfig {
    App(AppConfig),
    Domain(DomainConfig),
}

impl ValidateConfig for UnitConfig {
    fn validate_config(&self) -> Result<(), String> {
        match self {
            UnitConfig::App(config) => config.validate_config(),
            UnitConfig::Domain(config) => config.validate_config(),
        }
    }
}

impl UnitConfig {
    pub fn load(unit_dir: &Path) -> Result<Self, DeployError> {
        let path = unit_dir.join("config.yaml");
        let unit = load_config(&path).map_err(|err| {
            if err.is_not_found() {
                DeployError::UnitNotFound
            } else {
                DeployError::UnitConfig(err)
            }
        })?;

        Ok(unit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ValidateConfig;

    #[test]
    fn parse_domain_unit_config() {
        let config: UnitConfig = serde_yaml::from_str(
            r#"
type: domain
proxy:
  type: cloudflare
https: proxy
routes:
  - path: /api
    target:
      kind: app
      unit: backend
"#,
        )
        .unwrap();

        assert!(matches!(config, UnitConfig::Domain(_)));
        assert!(config.validate_config().is_ok());
    }
}
