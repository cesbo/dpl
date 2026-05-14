pub mod app;
pub mod db;
pub mod domain;

use std::fs;

use app::AppConfig;
use db::{
    DbConfig,
    DbServerConfig,
};
use domain::DomainConfig;
use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

use crate::{
    MainContext,
    config::{
        ConfigError,
        ValidateConfig,
        load_config,
        save_config,
    },
};

#[derive(Debug, Error)]
pub enum UnitConfigError {
    #[error("invalid unit name '{name}'")]
    InvalidName { name: String },

    #[error("unit '{name}' not found")]
    NotFound { name: String },

    #[error(transparent)]
    Config(#[from] ConfigError),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UnitConfig {
    App(AppConfig),
    Db(DbConfig),
    DbServer(DbServerConfig),
    Domain(DomainConfig),
}

impl ValidateConfig for UnitConfig {
    fn validate_config(&self) -> Result<(), String> {
        match self {
            UnitConfig::App(config) => config.validate_config(),
            UnitConfig::Db(config) => config.validate_config(),
            UnitConfig::DbServer(config) => config.validate_config(),
            UnitConfig::Domain(config) => config.validate_config(),
        }
    }
}

impl UnitConfig {
    pub fn load(ctx: &MainContext, name: &str) -> Result<Self, UnitConfigError> {
        if !crate::validate::resource_name(name) {
            return Err(UnitConfigError::InvalidName {
                name: name.to_string(),
            });
        }

        let path = ctx.base().join(name).join("config.yaml");
        load_config(&path).map_err(|err| {
            if err.is_not_found() {
                UnitConfigError::NotFound {
                    name: name.to_string(),
                }
            } else {
                UnitConfigError::Config(err)
            }
        })
    }

    pub fn save(&self, ctx: &MainContext, name: &str) -> Result<(), UnitConfigError> {
        if !crate::validate::resource_name(name) {
            return Err(UnitConfigError::InvalidName {
                name: name.to_string(),
            });
        }

        let dir = ctx.base().join(name);
        fs::create_dir_all(&dir).map_err(|err| UnitConfigError::Config(ConfigError::Write(err)))?;
        let path = dir.join("config.yaml");
        save_config(&path, self).map_err(UnitConfigError::Config)
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), String> {
        match self {
            UnitConfig::App(config) => config.validate_references(ctx)?,
            UnitConfig::Db(config) => config.validate_references(ctx)?,
            UnitConfig::DbServer(config) => config.validate_references(ctx)?,
            UnitConfig::Domain(_) => {}
        };

        Ok(())
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            UnitConfig::App(_) => "app",
            UnitConfig::Db(_) => "db",
            UnitConfig::DbServer(_) => "db-server",
            UnitConfig::Domain(_) => "domain",
        }
    }

    pub fn has_export(&self, key: &str) -> bool {
        match self {
            UnitConfig::App(_) => AppConfig::has_export(key),
            UnitConfig::Db(_) => DbConfig::has_export(key),
            _ => false,
        }
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        unit_name: &str,
        key: &str,
    ) -> Result<String, String> {
        match self {
            UnitConfig::App(config) => config.resolve_export(ctx, unit_name, key),
            UnitConfig::Db(config) => config.resolve_export(ctx, unit_name, key),
            _ => Err(format!("unit '{unit_name}' has no exports")),
        }
    }
}

pub fn validate_export(ctx: &MainContext, unit_name: &str, key: &str) -> Result<(), String> {
    let unit = UnitConfig::load(ctx, unit_name).map_err(|err| err.to_string())?;

    if unit.has_export(key) {
        Ok(())
    } else {
        Err("variable is not available".into())
    }
}

pub fn resolve_export(ctx: &MainContext, unit_name: &str, key: &str) -> Result<String, String> {
    UnitConfig::load(ctx, unit_name)
        .map_err(|err| err.to_string())?
        .resolve_export(ctx, unit_name, key)
}

/// Return all units satisfies `predicate`, sorted by name.
pub fn list_units<F>(ctx: &MainContext, predicate: F) -> Vec<(String, UnitConfig)>
where
    F: Fn(&UnitConfig) -> bool,
{
    let entries = match fs::read_dir(ctx.base()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut out: Vec<(String, UnitConfig)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if !file_type.is_dir() {
            continue;
        }

        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };

        if !crate::validate::resource_name(&name) {
            continue;
        }

        let Ok(config) = UnitConfig::load(ctx, &name) else {
            continue;
        };

        if predicate(&config) {
            out.push((name, config));
        }
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
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
hosts:
  - example.com
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
    fn parse_app_unit_config_with_databases() {
        let config: UnitConfig = serde_yaml::from_str(
            r#"
type: app
image: alpine
port: 8080
build: []
runtime:
  cmd: "./run"
databases:
  - main-db
  - cache-db
"#,
        )
        .unwrap();

        let UnitConfig::App(app) = config else {
            panic!("expected app variant");
        };
        assert_eq!(app.databases, vec!["main-db", "cache-db"]);
    }

    #[test]
    fn load_skips_reference_validation() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let app_dir = base.path().join("app-x");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("config.yaml"),
            "type: app\nimage: alpine\nport: 8080\nbuild: []\nruntime:\n  env:\n    OTHER: \"${nope:user}\"\n  cmd: ./run\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        // load skips reference validation: succeeds even with a missing ref.
        let unit = UnitConfig::load(&ctx, "app-x").unwrap();
        assert!(matches!(unit, UnitConfig::App(_)));

        // validate_references surfaces the missing unit.
        let err = unit.validate_references(&ctx).unwrap_err();
        assert!(
            err.contains("${nope:user}"),
            "expected error to mention '${{nope:user}}': {err}"
        );
    }

    #[test]
    fn save_roundtrip_db_server() {
        use tempfile::TempDir;

        use crate::deploy::unit::db::{
            DbServerConfig,
            DbServerEngine,
        };

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        let original = UnitConfig::DbServer(DbServerConfig {
            engine: DbServerEngine::Postgresql,
            version: "18-alpine".into(),
            secret: "pg-pass".into(),
        });
        original.save(&ctx, "pg-main").unwrap();

        let loaded = UnitConfig::load(&ctx, "pg-main").unwrap();
        let (UnitConfig::DbServer(a), UnitConfig::DbServer(b)) = (&original, &loaded) else {
            panic!("expected db-server variants");
        };
        assert_eq!(a, b);
    }

    #[test]
    fn save_rejects_invalid_name() {
        use tempfile::TempDir;

        use crate::deploy::unit::db::{
            DbServerConfig,
            DbServerEngine,
        };

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        let unit = UnitConfig::DbServer(DbServerConfig {
            engine: DbServerEngine::Postgresql,
            version: "18".into(),
            secret: "pg-pass".into(),
        });
        assert!(matches!(
            unit.save(&ctx, "Bad/Name"),
            Err(UnitConfigError::InvalidName { .. })
        ));
    }

    #[test]
    fn validate_export_unknown_unit() {
        let err = validate_export(&MainContext::default(), "nope", "user").unwrap_err();
        assert_eq!(err, "unit 'nope' not found");
    }

    #[test]
    fn validate_export_unknown_key_on_domain() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let dir = base.path().join("example-com");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.yaml"),
            "type: domain\nhosts:\n  - example.com\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let err = validate_export(&ctx, "example-com", "host").unwrap_err();
        assert_eq!(err, "variable is not available");
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
