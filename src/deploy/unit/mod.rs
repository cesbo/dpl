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
use serde::Deserialize;

use crate::{
    MainContext,
    config::{
        ConfigError,
        ValidateConfig,
        load_config,
    },
    deploy::{
        DeployError,
        env::EnvError,
    },
};

#[derive(Debug, Deserialize)]
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
    pub fn load(ctx: &MainContext, name: &str) -> Result<Self, DeployError> {
        if !crate::validate::resource_name(name) {
            return Err(DeployError::InvalidUnitName);
        }

        let path = ctx.base().join(name).join("config.yaml");
        load_config(&path).map_err(|err| {
            if err.is_not_found() {
                DeployError::UnitNotFound
            } else {
                DeployError::UnitConfig(err)
            }
        })
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), DeployError> {
        match self {
            UnitConfig::App(config) => {
                config
                    .validate_references(ctx)
                    .map_err(|info| ConfigError::Invalid(format!("app references: {info}")))?;
            }
            UnitConfig::Db(_) => {}
            UnitConfig::DbServer(_) => {}
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
            UnitConfig::Db(_) => DbConfig::has_export(key),
            _ => false,
        }
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        name: &str,
        key: &str,
    ) -> Result<String, EnvError> {
        match self {
            UnitConfig::Db(config) => config.resolve_export(ctx, name, key),
            _ => Err(EnvError::UnknownExport {
                unit: name.to_owned(),
                kind: self.kind(),
                key: key.to_owned(),
            }),
        }
    }
}

fn load_for_export(ctx: &MainContext, unit_name: &str) -> Result<UnitConfig, EnvError> {
    UnitConfig::load(ctx, unit_name).map_err(|err| match err {
        DeployError::UnitNotFound | DeployError::InvalidUnitName => EnvError::UnitNotFound {
            name: unit_name.to_owned(),
        },
        DeployError::UnitConfig(source) => EnvError::UnitConfig {
            name: unit_name.to_owned(),
            source,
        },
        other => EnvError::UnitConfig {
            name: unit_name.to_owned(),
            source: ConfigError::Invalid(other.to_string()),
        },
    })
}

pub(crate) fn validate_export(
    ctx: &MainContext,
    unit_name: &str,
    key: &str,
) -> Result<(), EnvError> {
    let unit = load_for_export(ctx, unit_name)?;
    if unit.has_export(key) {
        Ok(())
    } else {
        Err(EnvError::UnknownExport {
            unit: unit_name.to_owned(),
            kind: unit.kind(),
            key: key.to_owned(),
        })
    }
}

pub(crate) fn resolve_export(
    ctx: &MainContext,
    unit_name: &str,
    key: &str,
) -> Result<String, EnvError> {
    let unit = load_for_export(ctx, unit_name)?;
    unit.resolve_export(ctx, unit_name, key)
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
        let msg = err.to_string();
        assert!(
            msg.contains("nope") || format!("{err:?}").contains("nope"),
            "expected error to mention 'nope': {msg}"
        );
    }

    #[test]
    fn validate_export_unknown_unit() {
        let err = validate_export(&MainContext::default(), "nope", "user").unwrap_err();
        assert!(matches!(err, EnvError::UnitNotFound { ref name } if name == "nope"));
    }

    #[test]
    fn validate_export_unknown_key_on_non_db() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let dir = base.path().join("example-com");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("config.yaml"), "type: domain\n").unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let err = validate_export(&ctx, "example-com", "host").unwrap_err();
        assert!(matches!(
            err,
            EnvError::UnknownExport { kind, .. } if kind == "domain"
        ));
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
