use kdl::{
    KdlDocument,
    KdlEntry,
    KdlNode,
};
use percent_encoding::{
    AsciiSet,
    NON_ALPHANUMERIC,
    utf8_percent_encode,
};

use crate::{
    MainContext,
    config::{
        FromKdlNode,
        NodeError,
        ResourceName,
        SecretName,
        ValidateConfig,
        reject_children,
        set_field,
        string_node,
    },
    deploy::unit::UnitConfig,
    error::{
        Location,
        RefError,
    },
    kdl_args,
};

/// RFC 3986 *unreserved* set: encode everything except `A-Z a-z 0-9 - . _ ~`.
const USERINFO: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbServerConfig {
    pub engine: DbServerEngine,
    pub version: String,
    pub secret: SecretName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DbConfig {
    pub server: ResourceName,
    pub user: String,
    pub secret: SecretName,
}

impl DbConfig {
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        self.resolve_password(ctx)?;
        self.resolve_server(ctx)?.validate_references(ctx)?;
        Ok(())
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        unit_name: &str,
        key: &str,
    ) -> Result<String, RefError> {
        match key {
            "user" => Ok(self.user.clone()),
            "name" => Ok(unit_name.to_owned()),
            "password" => self.resolve_password(ctx),
            "host" => Ok(self.server.as_str().to_owned()),
            "port" => self
                .resolve_server(ctx)
                .map(|server| server.engine.default_port().to_string()),
            "url" => {
                let server = self.resolve_server(ctx)?;
                let password = self.resolve_password(ctx)?;
                Ok(format!(
                    "{scheme}://{user}:{password}@{host}:{port}/{db}",
                    scheme = server.engine.url_scheme(),
                    user = self.user,
                    password = utf8_percent_encode(&password, USERINFO),
                    host = self.server,
                    port = server.engine.default_port(),
                    db = unit_name,
                ))
            }
            _ => Err(RefError::UnknownExport {
                key: key.to_owned(),
            }),
        }
    }

    fn resolve_password(&self, ctx: &MainContext) -> Result<String, RefError> {
        ctx.resolve_secret(self.secret.as_str())
            .map_err(RefError::from)
            .map_err(|err| err.at(Location::field("secret")))
    }

    fn resolve_server(&self, ctx: &MainContext) -> Result<DbServerConfig, RefError> {
        UnitConfig::load(ctx, &self.server)
            .map_err(RefError::from)
            .and_then(|cfg| match cfg {
                UnitConfig::DbServer(server) => Ok(server),
                _ => Err(RefError::WrongUnitType {
                    unit: self.server.as_str().to_owned(),
                    expected: "db-server",
                }),
            })
            .map_err(|err| err.at(Location::field("server")))
    }
}

impl DbConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("db");
        let mut children = KdlDocument::new();
        children
            .nodes_mut()
            .push(string_node("server", self.server.as_str()));
        children.nodes_mut().push(string_node("user", &self.user));
        children
            .nodes_mut()
            .push(string_node("secret", self.secret.as_str()));
        node.set_children(children);
        node
    }
}

impl FromKdlNode for DbConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        kdl_args!(node)?;

        let mut server: Option<ResourceName> = None;
        let mut user: Option<String> = None;
        let mut secret: Option<SecretName> = None;

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "server" => set_field(&mut server, child)?,
                    "user" => set_field(&mut user, child)?,
                    "secret" => set_field(&mut secret, child)?,
                    _ => {
                        return Err(NodeError::UnknownField {
                            name: name.to_owned(),
                            span: child.span(),
                        });
                    }
                }
            }
        }

        Ok(DbConfig {
            server: server.ok_or(NodeError::MissingField {
                name: "server",
                span: node.span(),
            })?,
            user: user.ok_or(NodeError::MissingField {
                name: "user",
                span: node.span(),
            })?,
            secret: secret.ok_or(NodeError::MissingField {
                name: "secret",
                span: node.span(),
            })?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DbServerEngine {
    Postgresql,
    Mariadb,
}

impl DbServerEngine {
    pub fn as_str(&self) -> &'static str {
        match self {
            DbServerEngine::Postgresql => "postgresql",
            DbServerEngine::Mariadb => "mariadb",
        }
    }

    pub fn image(&self, version: &str) -> String {
        match self {
            DbServerEngine::Postgresql => format!("docker.io/library/postgres:{version}"),
            DbServerEngine::Mariadb => format!("docker.io/library/mariadb:{version}"),
        }
    }

    pub fn data_path(&self) -> &'static str {
        match self {
            DbServerEngine::Postgresql => "/var/lib/postgresql",
            DbServerEngine::Mariadb => "/var/lib/mysql",
        }
    }

    pub fn password_env(&self) -> &'static str {
        match self {
            DbServerEngine::Postgresql => "POSTGRES_PASSWORD",
            DbServerEngine::Mariadb => "MARIADB_ROOT_PASSWORD",
        }
    }

    pub fn default_version(&self) -> &'static str {
        match self {
            DbServerEngine::Postgresql => "18-alpine",
            DbServerEngine::Mariadb => "12",
        }
    }

    pub fn default_port(&self) -> u16 {
        match self {
            DbServerEngine::Postgresql => 5432,
            DbServerEngine::Mariadb => 3306,
        }
    }

    pub fn url_scheme(&self) -> &'static str {
        match self {
            DbServerEngine::Postgresql => "postgresql",
            DbServerEngine::Mariadb => "mysql",
        }
    }

    pub fn client_args(&self) -> &'static [&'static str] {
        match self {
            DbServerEngine::Postgresql => &[
                "psql",
                "-v",
                "ON_ERROR_STOP=1",
                "-U",
                "postgres",
                "-d",
                "postgres",
            ],
            DbServerEngine::Mariadb => &["mariadb", "-u", "root"],
        }
    }

    pub fn client_password_env(&self) -> &'static str {
        match self {
            DbServerEngine::Postgresql => "PGPASSWORD",
            DbServerEngine::Mariadb => "MYSQL_PWD",
        }
    }
}

impl FromKdlNode for DbServerEngine {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let variant = kdl_args!(node, engine: String)?;

        match variant.as_str() {
            "postgresql" => {
                reject_children(node)?;
                Ok(Self::Postgresql)
            }
            "mariadb" => {
                reject_children(node)?;
                Ok(Self::Mariadb)
            }
            other => Err(NodeError::UnknownVariant {
                field: node.name().value().to_owned(),
                value: other.to_owned(),
                span: node
                    .entries()
                    .first()
                    .map(|e| e.span())
                    .unwrap_or_else(|| node.span()),
            }),
        }
    }
}

impl DbServerConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("db-server");
        let mut children = KdlDocument::new();
        let mut engine = KdlNode::new("engine");
        engine
            .entries_mut()
            .push(KdlEntry::new(self.engine.as_str().to_owned()));
        children.nodes_mut().push(engine);
        children
            .nodes_mut()
            .push(string_node("version", &self.version));
        children
            .nodes_mut()
            .push(string_node("secret", self.secret.as_str()));
        node.set_children(children);
        node
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        self.resolve_password(ctx)?;
        Ok(())
    }

    fn resolve_password(&self, ctx: &MainContext) -> Result<String, RefError> {
        ctx.resolve_secret(self.secret.as_str())
            .map_err(RefError::from)
            .map_err(|err| err.at(Location::field("secret")))
    }
}

impl FromKdlNode for DbServerConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        kdl_args!(node)?;

        let mut engine: Option<DbServerEngine> = None;
        let mut version: Option<String> = None;
        let mut secret: Option<SecretName> = None;

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "engine" => set_field(&mut engine, child)?,
                    "version" => set_field(&mut version, child)?,
                    "secret" => set_field(&mut secret, child)?,
                    _ => {
                        return Err(NodeError::UnknownField {
                            name: name.to_owned(),
                            span: child.span(),
                        });
                    }
                }
            }
        }

        let Some(engine) = engine else {
            return Err(NodeError::MissingField {
                name: "engine",
                span: node.span(),
            });
        };

        let Some(version) = version else {
            return Err(NodeError::MissingField {
                name: "version",
                span: node.span(),
            });
        };

        let Some(secret) = secret else {
            return Err(NodeError::MissingField {
                name: "secret",
                span: node.span(),
            });
        };

        Ok(DbServerConfig {
            engine,
            version,
            secret,
        })
    }
}

impl ValidateConfig for DbServerConfig {
    fn validate_config(&self) -> Result<(), String> {
        if self.version.trim().is_empty() {
            return Err("db version must not be empty".into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use kdl::KdlDocument;

    use super::*;
    use crate::config::FieldError;

    fn parse_db(src: &str) -> Result<DbConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        DbConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_db_server(src: &str) -> Result<DbServerConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        DbServerConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    #[test]
    fn kdl_db_basic() {
        let cfg = parse_db(
            r#"
            db {
                server "pg-main"
                user "app1"
                secret "app1-pass"
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.server.as_str(), "pg-main");
        assert_eq!(cfg.user, "app1");
        assert_eq!(cfg.secret.as_str(), "app1-pass");
    }

    #[test]
    fn kdl_db_server_basic_postgres() {
        let cfg = parse_db_server(
            r#"
            db-server {
                engine "postgresql"
                version "18-alpine"
                secret "pg-pass"
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.engine, DbServerEngine::Postgresql);
        assert_eq!(cfg.version, "18-alpine");
        assert_eq!(cfg.secret.as_str(), "pg-pass");
    }

    #[test]
    fn kdl_db_server_basic_mariadb() {
        let cfg = parse_db_server(
            r#"
            db-server {
                engine "mariadb"
                version "12"
                secret "maria-pass"
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.engine, DbServerEngine::Mariadb);
    }

    #[test]
    fn kdl_db_server_unknown_engine() {
        let err = parse_db_server(
            r#"
            db-server {
                engine "sqlite"
                version "1"
                secret "s"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::UnknownVariant { field, value, .. }
                    if *field == "engine" && value == "sqlite",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_db_missing_field() {
        let err = parse_db(
            r#"
            db {
                server "pg-main"
                user "app1"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "secret"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_db_unknown_field() {
        let err = parse_db(
            r#"
            db {
                server "pg-main"
                user "app1"
                secret "s"
                host "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "host"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_db_duplicate_field() {
        let err = parse_db(
            r#"
            db {
                server "pg-main"
                server "pg-other"
                user "app1"
                secret "s"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "server"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_db_wrapper_with_args_rejected() {
        let err = parse_db(
            r#"
            db "stray" {
                server "pg-main"
                user "app1"
                secret "s"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_db_field_not_a_string() {
        let err = parse_db(
            r#"
            db {
                server 5
                user "app1"
                secret "s"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "server",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_db_field_with_children_rejected() {
        let err = parse_db(
            r#"
            db {
                server "pg-main"
                user "app1"
                secret "s" { extra }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::HasChildren { .. },
                    ..
                } if name == "secret",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn reject_empty_version() {
        let config = DbServerConfig {
            engine: DbServerEngine::Postgresql,
            version: " ".into(),
            secret: SecretName::new("pg-pass").unwrap(),
        };
        assert!(config.validate_config().is_err());
    }

    #[test]
    fn db_config_rejects_invalid_secret_at_parse() {
        let err = parse_db(
            r#"
            db {
                server "pg-main"
                user "app1"
                secret "Bad/Name/"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "secret",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn db_server_config_rejects_invalid_secret_at_parse() {
        let err = parse_db_server(
            r#"
            db-server {
                engine "postgresql"
                version "18"
                secret "Bad/Name/"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "secret",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn db_config_rejects_invalid_server_at_parse() {
        let err = parse_db(
            r#"
            db {
                server "Bad/Name"
                user "app1"
                secret "app1-pass"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "server",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn db_resolve_export_unknown_key() {
        let config = DbConfig {
            server: ResourceName::new("pg-main").unwrap(),
            user: "app1".into(),
            secret: SecretName::new("app1-pass").unwrap(),
        };
        let err = config
            .resolve_export(&MainContext::default(), "app-db", "unknown")
            .unwrap_err();
        assert!(
            matches!(&err, RefError::UnknownExport { key } if key == "unknown"),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn db_resolve_export_postgres() {
        use std::fs;

        use tempfile::TempDir;

        use crate::secret::MasterKey;

        let base = TempDir::new().unwrap();
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
        key.encrypt_to_file("app1-pass", "top$ecret&").unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };
        let config = DbConfig {
            server: ResourceName::new("pg-main").unwrap(),
            user: "app1".into(),
            secret: SecretName::new("app1-pass").unwrap(),
        };

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "name").unwrap(),
            "app-db"
        );

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "user").unwrap(),
            "app1"
        );

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "password").unwrap(),
            "top$ecret&"
        );

        assert_eq!(
            config
                .resolve_export(&MainContext::default(), "app-db", "host")
                .unwrap(),
            "pg-main"
        );

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "port").unwrap(),
            "5432"
        );

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "url").unwrap(),
            "postgresql://app1:top%24ecret%26@pg-main:5432/app-db"
        );
    }

    #[test]
    fn db_resolve_export_mariadb() {
        use std::fs;

        use tempfile::TempDir;

        use crate::secret::MasterKey;

        let base = TempDir::new().unwrap();
        let server_dir = base.path().join("maria-main");
        fs::create_dir_all(&server_dir).unwrap();
        fs::write(
            server_dir.join("config.kdl"),
            r#"
db-server {
    engine "mariadb"
    version "12"
    secret "maria-pass"
}
"#,
        )
        .unwrap();

        let key = MasterKey::generate(base.path());
        key.save().unwrap();
        key.encrypt_to_file("app1-pass", "top$ecret&").unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };
        let config = DbConfig {
            server: ResourceName::new("maria-main").unwrap(),
            user: "app1".into(),
            secret: SecretName::new("app1-pass").unwrap(),
        };

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "name").unwrap(),
            "app-db"
        );

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "user").unwrap(),
            "app1"
        );

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "password").unwrap(),
            "top$ecret&"
        );

        assert_eq!(
            config
                .resolve_export(&MainContext::default(), "app-db", "host")
                .unwrap(),
            "maria-main"
        );

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "port").unwrap(),
            "3306"
        );

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "url").unwrap(),
            "mysql://app1:top%24ecret%26@maria-main:3306/app-db"
        );
    }

    #[test]
    fn metadata_postgres() {
        let engine = DbServerEngine::Postgresql;
        assert_eq!(engine.as_str(), "postgresql");
        assert_eq!(
            engine.image("18-alpine"),
            "docker.io/library/postgres:18-alpine"
        );
        assert_eq!(engine.data_path(), "/var/lib/postgresql");
        assert_eq!(engine.password_env(), "POSTGRES_PASSWORD");
        assert_eq!(engine.default_version(), "18-alpine");
        assert_eq!(engine.default_port(), 5432);
        assert_eq!(engine.url_scheme(), "postgresql");

        let engine = DbServerEngine::Mariadb;
        assert_eq!(engine.as_str(), "mariadb");
        assert_eq!(engine.image("12"), "docker.io/library/mariadb:12");
        assert_eq!(engine.data_path(), "/var/lib/mysql");
        assert_eq!(engine.password_env(), "MARIADB_ROOT_PASSWORD");
        assert_eq!(engine.default_version(), "12");
        assert_eq!(engine.default_port(), 3306);
        assert_eq!(engine.url_scheme(), "mysql");
    }
}
