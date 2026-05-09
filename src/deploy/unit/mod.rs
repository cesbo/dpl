pub mod app;
pub mod db;
pub mod domain;

use app::AppConfig;
use db::DbServerConfig;
use domain::DomainConfig;
use serde::Deserialize;

use crate::{
    MainContext,
    config::{
        ConfigError,
        ValidateConfig,
        load_config,
    },
    deploy::DeployError,
};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UnitConfig {
    App(AppConfig),
    DbServer(DbServerConfig),
    Domain(DomainConfig),
}

impl ValidateConfig for UnitConfig {
    fn validate_config(&self) -> Result<(), String> {
        match self {
            UnitConfig::App(config) => config.validate_config(),
            UnitConfig::DbServer(config) => config.validate_config(),
            UnitConfig::Domain(config) => config.validate_config(),
        }
    }
}

impl UnitConfig {
    pub fn load(ctx: &MainContext, name: &str) -> Result<Self, DeployError> {
        if !crate::validate::resource_name(name) {
            return Err(DeployError::InvalidUnitName);
        }

        let path = ctx.base().join(name).join("config.yaml");
        let unit = load_config(&path).map_err(|err| {
            if err.is_not_found() {
                DeployError::UnitNotFound
            } else {
                DeployError::UnitConfig(err)
            }
        })?;

        match &unit {
            UnitConfig::App(config) => config
                .validate_references(ctx)
                .map_err(|info| ConfigError::Invalid(format!("app references: {info}")))?,
            UnitConfig::DbServer(_) => {}
            UnitConfig::Domain(_) => {}
        }

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

    #[test]
    fn parse_db_server_unit_config() {
        let config: UnitConfig = serde_yaml::from_str(
            r#"
type: db-server
engine: postgresql
version: "18"
secret: pg-pass
"#,
        )
        .unwrap();

        let UnitConfig::DbServer(db) = &config else {
            panic!("expected db-server variant");
        };
        assert_eq!(db.version, "18");
        assert_eq!(db.secret, "pg-pass");
        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn parse_app_unit_config_with_secret_template() {
        let config: UnitConfig = serde_yaml::from_str(
            r#"
type: app
image: alpine
port: 8080
build: []
runtime:
  env:
    PLAIN: "hello"
    SECRET_KEY: "${secret:my-key}"
    DATABASE_URL: "postgres://app:${secret:my-key}@db/app"
  cmd: "./run"
"#,
        )
        .unwrap();

        let UnitConfig::App(app) = config else {
            panic!("expected app variant");
        };
        assert_eq!(app.runtime.cmd, "./run");
        assert_eq!(app.runtime.env.validate_config(), Ok(()));
    }
}
