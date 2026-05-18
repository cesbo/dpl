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
use kdl::KdlDocument;
use miette::SourceSpan;
use thiserror::Error;

use crate::{
    MainContext,
    config::{
        ConfigError,
        FromKdlNode,
        NodeError,
        ValidateConfig,
    },
    error::{
        Location,
        RefError,
    },
};

#[derive(Debug, Error)]
pub enum UnitConfigError {
    #[error("invalid unit name '{name}'")]
    InvalidName { name: String },

    #[error("unit '{name}' not found")]
    NotFound { name: String },

    #[error("load config for unit '{name}'")]
    Config {
        name: String,
        #[source]
        source: ConfigError,
    },
}

impl From<UnitConfigError> for RefError {
    fn from(err: UnitConfigError) -> Self {
        match err {
            UnitConfigError::NotFound { name } => RefError::UnknownUnit { name },
            UnitConfigError::InvalidName { name } => RefError::UnknownUnit { name },
            UnitConfigError::Config { name, source } => RefError::LoadConfig { name, source },
        }
    }
}

#[derive(Debug)]
pub enum UnitConfig {
    App(AppConfig),
    Db(DbConfig),
    DbServer(DbServerConfig),
    Domain(DomainConfig),
}

impl From<&UnitConfig> for KdlDocument {
    fn from(cfg: &UnitConfig) -> Self {
        let mut doc = KdlDocument::new();
        let node = match cfg {
            UnitConfig::App(c) => c.to_kdl_node(),
            UnitConfig::Db(c) => c.to_kdl_node(),
            UnitConfig::DbServer(c) => c.to_kdl_node(),
            UnitConfig::Domain(c) => c.to_kdl_node(),
        };
        doc.nodes_mut().push(node);
        doc
    }
}

impl TryFrom<&KdlDocument> for UnitConfig {
    type Error = NodeError;

    fn try_from(doc: &KdlDocument) -> Result<Self, NodeError> {
        let nodes = doc.nodes();
        let Some(first) = nodes.first() else {
            return Err(NodeError::MissingField {
                name: "unit type",
                span: SourceSpan::new(0.into(), 0),
            });
        };
        if let Some(extra) = nodes.get(1) {
            return Err(NodeError::UnknownField {
                name: extra.name().value().to_owned(),
                span: extra.span(),
            });
        }

        let name = first.name().value();
        let variant_span = first.name().span();
        match name {
            "app" => AppConfig::from_kdl_node(first).map(UnitConfig::App),
            "db" => DbConfig::from_kdl_node(first).map(UnitConfig::Db),
            "db-server" => DbServerConfig::from_kdl_node(first).map(UnitConfig::DbServer),
            "domain" => DomainConfig::from_kdl_node(first).map(UnitConfig::Domain),
            other => Err(NodeError::UnknownVariant {
                field: "unit type".to_owned(),
                value: other.to_owned(),
                span: variant_span,
            }),
        }
    }
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

        let wrap = |source| UnitConfigError::Config {
            name: name.to_string(),
            source,
        };

        let path = ctx.base().join(name).join("config.kdl");
        let content = fs::read_to_string(&path).map_err(|err| {
            if err.kind() == io::ErrorKind::NotFound {
                UnitConfigError::NotFound {
                    name: name.to_string(),
                }
            } else {
                wrap(ConfigError::Read(err))
            }
        })?;

        let doc: KdlDocument = content
            .parse()
            .map_err(|e| wrap(ConfigError::Parse(Box::new(e))))?;
        let config = Self::try_from(&doc).map_err(|e| wrap(ConfigError::Semantic(Box::new(e))))?;
        config
            .validate_config()
            .map_err(|e| wrap(ConfigError::Invalid(e)))?;
        Ok(config)
    }

    pub fn save(&self, ctx: &MainContext, name: &str) -> Result<(), UnitConfigError> {
        if !crate::validate::resource_name(name) {
            return Err(UnitConfigError::InvalidName {
                name: name.to_string(),
            });
        }

        let wrap = |source| UnitConfigError::Config {
            name: name.to_string(),
            source,
        };

        let dir = ctx.base().join(name);
        fs::create_dir_all(&dir).map_err(|err| wrap(ConfigError::Write(err)))?;

        let path = dir.join("config.kdl");
        let doc: KdlDocument = self.into();
        fs::write(&path, doc.to_string()).map_err(|err| wrap(ConfigError::Write(err)))?;
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
        unit_name: &str,
        key: &str,
    ) -> Result<String, RefError> {
        match self {
            UnitConfig::App(config) => config.resolve_export(ctx, unit_name, key),
            UnitConfig::Db(config) => config.resolve_export(ctx, unit_name, key),
            _ => Err(RefError::UnknownExport {
                key: key.to_owned(),
            }),
        }
    }
}

pub fn resolve_export(ctx: &MainContext, unit_name: &str, key: &str) -> Result<String, RefError> {
    UnitConfig::load(ctx, unit_name)
        .map_err(RefError::from)
        .and_then(|cfg| cfg.resolve_export(ctx, unit_name, key))
        .map_err(|err| err.at(Location::unit(unit_name)))
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

    fn parse_unit_doc(src: &str) -> Result<UnitConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        UnitConfig::try_from(&doc)
    }

    #[test]
    fn kdl_dispatch_app() {
        let cfg = parse_unit_doc(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap();
        assert!(matches!(cfg, UnitConfig::App(_)));
    }

    #[test]
    fn kdl_dispatch_db() {
        let cfg = parse_unit_doc(
            r#"
            db {
                server "pg-main"
                user "app1"
                secret "app1-pass"
            }
            "#,
        )
        .unwrap();
        assert!(matches!(cfg, UnitConfig::Db(_)));
    }

    #[test]
    fn kdl_dispatch_db_server() {
        let cfg = parse_unit_doc(
            r#"
            db-server {
                engine "postgresql"
                version "18-alpine"
                secret "pg-pass"
            }
            "#,
        )
        .unwrap();
        assert!(matches!(cfg, UnitConfig::DbServer(_)));
    }

    #[test]
    fn kdl_dispatch_domain() {
        let cfg = parse_unit_doc(
            r#"
            domain {
                host "example.com"
            }
            "#,
        )
        .unwrap();
        assert!(matches!(cfg, UnitConfig::Domain(_)));
    }

    #[test]
    fn kdl_dispatch_empty_document() {
        let err = parse_unit_doc("").unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "unit type"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_dispatch_extra_top_level_node() {
        let err = parse_unit_doc(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
            }
            stray
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "stray"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_dispatch_unknown_unit_type() {
        let err = parse_unit_doc(r#"service { foo "bar" }"#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::UnknownVariant { field, value, .. }
                    if field == "unit type" && value == "service",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn parse_domain_unit_config() {
        let config = parse_unit_doc(
            r#"
            domain {
                host "example.com"
                proxy "cloudflare"
                route reverse_proxy "/api" {
                    target "${backend:url}"
                }
            }
            "#,
        )
        .unwrap();

        assert!(matches!(config, UnitConfig::Domain(_)));
        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn parse_app_unit_config_with_databases() {
        let config = parse_unit_doc(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
                database "main-db"
                database "cache-db"
            }
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
            app_dir.join("config.kdl"),
            r#"
app {
    image "alpine"
    port 8080
    runtime {
        env { OTHER "${nope:user}" }
        cmd "./run"
    }
}
"#,
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        // load skips reference validation: succeeds even with a missing ref.
        let unit = UnitConfig::load(&ctx, "app-x").unwrap();
        assert!(matches!(unit, UnitConfig::App(_)));

        // validate_references surfaces the missing unit through the typed chain.
        let err = unit.validate_references(&ctx).unwrap_err();
        let RefError::At {
            location: field_loc,
            inner: env_inner,
        } = err
        else {
            panic!("expected outer At(Field), got {err:?}");
        };
        assert!(
            matches!(&field_loc, Location::Field { path } if path == "runtime.env.OTHER"),
            "unexpected outer location: {field_loc:?}",
        );
        let RefError::At {
            location: token_loc,
            inner: unit_layer,
        } = *env_inner
        else {
            panic!("expected At(Token) below field");
        };
        assert!(
            matches!(&token_loc, Location::Token { raw } if raw == "${nope:user}"),
            "unexpected token location: {token_loc:?}",
        );
        let RefError::At {
            location: unit_loc,
            inner: leaf,
        } = *unit_layer
        else {
            panic!("expected At(Unit) below token");
        };
        assert!(
            matches!(&unit_loc, Location::Unit { name } if name == "nope"),
            "unexpected unit location: {unit_loc:?}",
        );
        assert!(
            matches!(*leaf, RefError::UnknownUnit { ref name } if name == "nope"),
            "unexpected leaf: {leaf:?}",
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
            app_dir.join("config.kdl"),
            r#"
app {
    image "alpine"
    port 8080
    runtime {
        env { X "${db-test:password}" }
        cmd "./run"
    }
}
"#,
        )
        .unwrap();

        let db_dir = base.path().join("db-test");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(
            db_dir.join("config.kdl"),
            r#"
db {
    server "pg-main"
    user "app1"
    secret "foo-db-test-password"
}
"#,
        )
        .unwrap();

        let server_dir = base.path().join("pg-main");
        fs::create_dir_all(&server_dir).unwrap();
        fs::write(
            server_dir.join("config.kdl"),
            r#"
db-server {
    engine "postgresql"
    version "18"
    secret "pg-pass"
}
"#,
        )
        .unwrap();

        let key = MasterKey::generate(base.path());
        key.save().unwrap();
        // Provide pg-main's password so recursive server-validation succeeds.
        key.encrypt_to_file("pg-pass", "pg-secret").unwrap();
        // `foo-db-test-password` is intentionally absent — this is the leaf failure.

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };

        let foo = UnitConfig::load(&ctx, "foo").unwrap();
        let err = foo.validate_references(&ctx).unwrap_err();

        // Expected chain (outer → inner):
        //   At(Field "runtime.env.X")
        //     At(Token "${db-test:password}")
        //       At(Unit "db-test")
        //         At(Field "secret")
        //           Secret(NotFound { name: "foo-db-test-password" })
        let RefError::At {
            location: l1,
            inner: i1,
        } = err
        else {
            panic!("layer 1: expected At, got {err:?}");
        };
        assert!(matches!(&l1, Location::Field { path } if path == "runtime.env.X"));

        let RefError::At {
            location: l2,
            inner: i2,
        } = *i1
        else {
            panic!("layer 2: expected At");
        };
        assert!(matches!(&l2, Location::Token { raw } if raw == "${db-test:password}"));

        let RefError::At {
            location: l3,
            inner: i3,
        } = *i2
        else {
            panic!("layer 3: expected At");
        };
        assert!(matches!(&l3, Location::Unit { name } if name == "db-test"));

        let RefError::At {
            location: l4,
            inner: i4,
        } = *i3
        else {
            panic!("layer 4: expected At");
        };
        assert!(matches!(&l4, Location::Field { path } if path == "secret"));

        assert!(
            matches!(*i4, RefError::Secret(SecretError::NotFound { ref name }) if name == "foo-db-test-password"),
            "unexpected leaf: {i4:?}",
        );
    }

    #[test]
    fn resolve_export_unknown_unit() {
        let err = resolve_export(&MainContext::default(), "nope", "user").unwrap_err();
        let RefError::At { location, inner } = err else {
            panic!("expected At wrapper, got {err:?}");
        };
        assert!(matches!(&location, Location::Unit { name } if name == "nope"));
        assert!(matches!(*inner, RefError::UnknownUnit { ref name } if name == "nope"));
    }

    #[test]
    fn resolve_export_on_domain_has_no_exports() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let dir = base.path().join("example-com");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.kdl"),
            r#"
domain {
    host "example.com"
}
"#,
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let err = resolve_export(&ctx, "example-com", "host").unwrap_err();
        let RefError::At { location, inner } = err else {
            panic!("expected At wrapper, got {err:?}");
        };
        assert!(matches!(&location, Location::Unit { name } if name == "example-com"));
        assert!(matches!(*inner, RefError::UnknownExport { ref key } if key == "host"));
    }

    #[test]
    fn parse_app_unit_config_with_secret_template() {
        let config = parse_unit_doc(
            r#"
            app {
                image "alpine"
                port 8080
                runtime {
                    env {
                        PLAIN "hello"
                        SECRET_KEY "${secret:my-key}"
                        DATABASE_URL "postgres://app:${secret:my-key}@db/app"
                    }
                    cmd "./run"
                }
            }
            "#,
        )
        .unwrap();

        let UnitConfig::App(app) = config else {
            panic!("expected app variant");
        };
        assert_eq!(app.runtime.cmd, "./run");
    }
}
