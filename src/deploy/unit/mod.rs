pub mod app;
pub mod db;
pub mod domain;

use std::{
    fs,
    io,
};

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

use crate::{
    MainContext,
    config::{
        ConfigError,
        ResourceName,
    },
    error::{
        Location,
        RefError,
    },
};

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum UnitConfig {
    App(AppConfig),
    Db(DbConfig),
    DbServer(DbServerConfig),
    Domain(DomainConfig),
}

impl UnitConfig {
    pub fn load(ctx: &MainContext, name: &ResourceName) -> Result<Self, ConfigError> {
        let unit_dir = name.unit_dir(ctx);
        let path = unit_dir.join("config.yaml");
        let content = fs::read_to_string(&path).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                ConfigError::NotFound {
                    name: name.to_string(),
                }
            } else {
                ConfigError::Read {
                    name: name.to_string(),
                    source: err,
                }
            }
        })?;
        serde_yaml::from_str(&content).map_err(|err| ConfigError::Parse {
            name: name.to_string(),
            source: err,
        })
    }

    pub fn save(&self, ctx: &MainContext, name: &ResourceName) -> Result<(), ConfigError> {
        let unit_dir = name.unit_dir(ctx);
        fs::create_dir_all(&unit_dir).map_err(|err| ConfigError::Write {
            name: name.to_string(),
            source: err,
        })?;
        let yaml = serde_yaml::to_string(self).map_err(|err| ConfigError::Serialize {
            name: name.to_string(),
            source: err,
        })?;
        fs::write(unit_dir.join("config.yaml"), yaml).map_err(|err| ConfigError::Write {
            name: name.to_string(),
            source: err,
        })?;
        Ok(())
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        match self {
            UnitConfig::App(config) => config.validate_references(ctx),
            UnitConfig::Db(config) => config.validate_references(ctx),
            UnitConfig::DbServer(config) => config.validate_references(ctx),
            UnitConfig::Domain(config) => config.validate_references(ctx),
        }
    }

    pub(crate) fn kind(&self) -> &'static str {
        match self {
            UnitConfig::App(_) => "app",
            UnitConfig::Db(_) => "db",
            UnitConfig::DbServer(_) => "db-server",
            UnitConfig::Domain(_) => "domain",
        }
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        unit_name: &ResourceName,
        key: &str,
    ) -> Result<String, RefError> {
        match self {
            UnitConfig::App(config) => config.resolve_export(ctx, unit_name, key),
            UnitConfig::Db(config) => config.resolve_export(ctx, unit_name, key),
            _ => Err(RefError::unknown_export(key)),
        }
    }
}

pub fn resolve_export(
    ctx: &MainContext,
    unit_name: &ResourceName,
    key: &str,
) -> Result<String, RefError> {
    UnitConfig::load(ctx, unit_name)
        .map_err(RefError::from)
        .and_then(|cfg| cfg.resolve_export(ctx, unit_name, key))
        .map_err(|err| err.at(Location::unit(unit_name.as_str())))
}

/// Return all units satisfies `predicate`, sorted by name.
pub fn list_units<F>(ctx: &MainContext, predicate: F) -> Vec<(ResourceName, UnitConfig)>
where
    F: Fn(&UnitConfig) -> bool,
{
    let entries = match fs::read_dir(ctx.base()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut out: Vec<(ResourceName, UnitConfig)> = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if !file_type.is_dir() {
            continue;
        }

        let file_name = entry.file_name();
        let Some(raw) = file_name.to_str() else {
            continue;
        };

        let Ok(unit_name) = ResourceName::new(raw) else {
            continue;
        };

        let Ok(config) = UnitConfig::load(ctx, &unit_name) else {
            continue;
        };

        if predicate(&config) {
            out.push((unit_name, config));
        }
    }

    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::SecretName,
        error::RefErrorKind,
    };

    #[test]
    fn parse_domain_unit_config() {
        let config: UnitConfig = serde_yaml::from_str(
            r#"
type: domain
hosts:
  - example.com
proxy:
  type: cloudflare
routes:
  - location: /api
    kind: reverse_proxy
    target: "${backend:url}"
"#,
        )
        .unwrap();

        assert!(matches!(config, UnitConfig::Domain(_)));
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
        assert_eq!(db.secret.as_str(), "pg-pass");
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
            "type: app\nimage: alpine\nport: 8080\nbuilds: []\nruntime:\n  env:\n    OTHER: \"${nope:user}\"\n  cmd: ./run\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        // load skips reference validation: succeeds even with a missing ref.
        let unit = UnitConfig::load(&ctx, &ResourceName::new("app-x").unwrap()).unwrap();
        assert!(matches!(unit, UnitConfig::App(_)));

        // validate_references surfaces the missing unit through the typed chain.
        let err = unit.validate_references(&ctx).unwrap_err();
        // Trail is innermost-first: unit → token → field.
        assert!(
            matches!(&err.trail[0], Location::Unit { name } if name == "nope"),
            "unexpected unit location: {:?}",
            err.trail[0],
        );
        assert!(
            matches!(&err.trail[1], Location::Token { raw } if raw == "${nope:user}"),
            "unexpected token location: {:?}",
            err.trail[1],
        );
        assert!(
            matches!(&err.trail[2], Location::Field { path } if path == "runtime.env.OTHER"),
            "unexpected field location: {:?}",
            err.trail[2],
        );
        assert!(
            matches!(err.kind, RefErrorKind::UnknownUnit { ref name } if name == "nope"),
            "unexpected kind: {:?}",
            err.kind,
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
        let unit_name = ResourceName::new("pg-main").unwrap();

        let original = UnitConfig::DbServer(DbServerConfig {
            engine: DbServerEngine::Postgresql,
            version: "18-alpine".into(),
            secret: SecretName::new("pg-pass").unwrap(),
        });
        original.save(&ctx, &unit_name).unwrap();

        let loaded = UnitConfig::load(&ctx, &unit_name).unwrap();
        let (UnitConfig::DbServer(a), UnitConfig::DbServer(b)) = (&original, &loaded) else {
            panic!("expected db-server variants");
        };
        assert_eq!(a, b);
    }

    #[test]
    fn validate_references_full_chain() {
        use std::fs;

        use tempfile::TempDir;

        use crate::secret::{
            MasterKey,
            SecretError,
        };

        // Layout:
        //   app `foo` → runtime.env.X = "${db-test:password}"
        //   db  `db-test` → server: pg-main, secret: foo-db-test-password
        //   db-server `pg-main` (referenced by db-test, with its own secret to satisfy
        //   recursive validation — gets a real secret on disk so the only missing
        //   piece is `foo-db-test-password`).
        let base = TempDir::new().unwrap();

        let app_dir = base.path().join("foo");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("config.yaml"),
            "type: app\nimage: alpine\nport: 8080\nbuilds: []\nruntime:\n  env:\n    X: \"${db-test:password}\"\n  cmd: ./run\n",
        )
        .unwrap();

        let db_dir = base.path().join("db-test");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(
            db_dir.join("config.yaml"),
            "type: db\nserver: pg-main\nuser: app1\nsecret: foo-db-test-password\n",
        )
        .unwrap();

        let server_dir = base.path().join("pg-main");
        fs::create_dir_all(&server_dir).unwrap();
        fs::write(
            server_dir.join("config.yaml"),
            "type: db-server\nengine: postgresql\nversion: \"18\"\nsecret: pg-pass\n",
        )
        .unwrap();

        let key = MasterKey::generate(base.path());
        key.save().unwrap();
        // Provide pg-main's password so recursive server-validation succeeds.
        key.encrypt_to_file(&SecretName::new("pg-pass").unwrap(), "pg-secret")
            .unwrap();
        // `foo-db-test-password` is intentionally absent — this is the leaf failure.

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };

        let foo = UnitConfig::load(&ctx, &ResourceName::new("foo").unwrap()).unwrap();
        let err = foo.validate_references(&ctx).unwrap_err();

        // Expected trail, innermost-first:
        //   field "secret" → unit "db-test" → token "${db-test:password}"
        //     → field "runtime.env.X"
        // with kind Secret(NotFound { name: "foo-db-test-password" }).
        assert!(matches!(&err.trail[0], Location::Field { path } if path == "secret"));
        assert!(matches!(&err.trail[1], Location::Unit { name } if name == "db-test"));
        assert!(matches!(&err.trail[2], Location::Token { raw } if raw == "${db-test:password}"));
        assert!(matches!(&err.trail[3], Location::Field { path } if path == "runtime.env.X"));
        assert!(
            matches!(err.kind, RefErrorKind::Secret(SecretError::NotFound { ref name }) if name == "foo-db-test-password"),
            "unexpected kind: {:?}",
            err.kind,
        );
    }

    #[test]
    fn validate_references_databases_chain() {
        use std::fs;

        use tempfile::TempDir;

        use crate::secret::{
            MasterKey,
            SecretError,
        };

        // Layout (the db-dependency recursion path, distinct from the leaf
        // env/token failure exercised by `validate_references_full_chain`):
        //   app `foo` → runtime.env.DB = "${db-test:url}"
        //   db  `db-test` → server: pg-main (with its own secret present)
        //   db-server `pg-main` → secret: pg-pass (intentionally MISSING)
        // The url renders fine (db-test's own secret is present), so the env
        // `resolve` passes; the failure surfaces only when the dependency walk
        // descends into db-test → pg-main. The trail must keep every hop.
        let base = TempDir::new().unwrap();

        let app_dir = base.path().join("foo");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("config.yaml"),
            "type: app\nimage: alpine\nport: 8080\nbuilds: []\nruntime:\n  env:\n    DB: \"${db-test:url}\"\n  cmd: ./run\n",
        )
        .unwrap();

        let db_dir = base.path().join("db-test");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(
            db_dir.join("config.yaml"),
            "type: db\nserver: pg-main\nuser: app1\nsecret: db-test-password\n",
        )
        .unwrap();

        let server_dir = base.path().join("pg-main");
        fs::create_dir_all(&server_dir).unwrap();
        fs::write(
            server_dir.join("config.yaml"),
            "type: db-server\nengine: postgresql\nversion: \"18\"\nsecret: pg-pass\n",
        )
        .unwrap();

        let key = MasterKey::generate(base.path());
        key.save().unwrap();
        // db-test's own secret is present; only pg-main's `pg-pass` is missing,
        // so the failure originates two hops deep, inside db-server validation.
        key.encrypt_to_file(&SecretName::new("db-test-password").unwrap(), "db-secret")
            .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };

        let foo = UnitConfig::load(&ctx, &ResourceName::new("foo").unwrap()).unwrap();
        let err = foo.validate_references(&ctx).unwrap_err();

        // Expected trail, innermost-first:
        //   field "secret" → unit "pg-main" → unit "db-test"
        assert!(matches!(&err.trail[0], Location::Field { path } if path == "secret"));
        assert!(matches!(&err.trail[1], Location::Unit { name } if name == "pg-main"));
        assert!(matches!(&err.trail[2], Location::Unit { name } if name == "db-test"));
        assert!(
            matches!(err.kind, RefErrorKind::Secret(SecretError::NotFound { ref name }) if name == "pg-pass"),
            "unexpected kind: {:?}",
            err.kind,
        );
    }

    #[test]
    fn resolve_export_unknown_unit() {
        let err = resolve_export(
            &MainContext::default(),
            &ResourceName::new("nope").unwrap(),
            "user",
        )
        .unwrap_err();
        assert!(matches!(&err.trail[0], Location::Unit { name } if name == "nope"));
        assert!(matches!(err.kind, RefErrorKind::UnknownUnit { ref name } if name == "nope"));
    }

    #[test]
    fn resolve_export_on_domain_has_no_exports() {
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
        let err =
            resolve_export(&ctx, &ResourceName::new("example-com").unwrap(), "host").unwrap_err();
        assert!(matches!(&err.trail[0], Location::Unit { name } if name == "example-com"));
        assert!(matches!(err.kind, RefErrorKind::UnknownExport { ref key } if key == "host"));
    }

    #[test]
    fn parse_app_unit_config_with_secret_template() {
        let config: UnitConfig = serde_yaml::from_str(
            r#"
type: app
image: alpine
port: 8080
builds: []
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
    }
}
