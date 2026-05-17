use kdl::KdlNode;
use percent_encoding::{
    AsciiSet,
    NON_ALPHANUMERIC,
    utf8_percent_encode,
};
use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    MainContext,
    config::{
        FromKdlNode,
        NodeError,
        ValidateConfig,
        parse_string_child,
        set_field,
    },
    deploy::unit::UnitConfig,
    error::{
        Location,
        RefError,
    },
    kdl_args,
    validate::{
        resource_name,
        secret_name,
    },
};

/// RFC 3986 *unreserved* set: encode everything except `A-Z a-z 0-9 - . _ ~`.
const USERINFO: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DbServerConfig {
    pub engine: DbServerEngine,
    pub version: String,
    pub secret: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DbConfig {
    pub server: String,
    pub user: String,
    pub secret: String,
}

impl ValidateConfig for DbConfig {
    fn validate_config(&self) -> Result<(), String> {
        if !resource_name(&self.server) {
            return Err(format!("invalid db server name '{}'", self.server));
        }

        if !resource_name(&self.user) {
            return Err(format!("invalid db user name '{}'", self.user));
        }

        if !secret_name(&self.secret) {
            return Err(format!("invalid secret name '{}'", self.secret));
        }

        Ok(())
    }
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
            "host" => Ok(self.server.clone()),
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
        ctx.resolve_secret(&self.secret)
            .map_err(RefError::from)
            .map_err(|err| err.at(Location::field("secret")))
    }

    fn resolve_server(&self, ctx: &MainContext) -> Result<DbServerConfig, RefError> {
        UnitConfig::load(ctx, &self.server)
            .map_err(RefError::from)
            .and_then(|cfg| match cfg {
                UnitConfig::DbServer(server) => Ok(server),
                _ => Err(RefError::WrongUnitType {
                    unit: self.server.clone(),
                    expected: "db-server",
                }),
            })
            .map_err(|err| err.at(Location::field("server")))
    }
}

impl TryFrom<&KdlNode> for DbConfig {
    type Error = NodeError;

    fn try_from(node: &KdlNode) -> Result<Self, Self::Error> {
        kdl_args!(node)?;

        let mut server: Option<String> = None;
        let mut user: Option<String> = None;
        let mut secret: Option<String> = None;

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "server" => set_field(&mut server, child, name)?,
                    "user" => set_field(&mut user, child, name)?,
                    "secret" => set_field(&mut secret, child, name)?,
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
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
    fn from_kdl_node(node: &KdlNode, name: &str) -> Result<Self, NodeError> {
        let value = parse_string_child(node).map_err(|source| NodeError::InvalidField {
            name: name.to_owned(),
            span: node.span(),
            source,
        })?;

        match value {
            "postgresql" => Ok(Self::Postgresql),
            "mariadb" => Ok(Self::Mariadb),
            other => Err(NodeError::UnknownVariant {
                field: name.to_owned(),
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
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        self.resolve_password(ctx)?;
        Ok(())
    }

    fn resolve_password(&self, ctx: &MainContext) -> Result<String, RefError> {
        ctx.resolve_secret(&self.secret)
            .map_err(RefError::from)
            .map_err(|err| err.at(Location::field("secret")))
    }
}

impl TryFrom<&KdlNode> for DbServerConfig {
    type Error = NodeError;

    fn try_from(node: &KdlNode) -> Result<Self, Self::Error> {
        kdl_args!(node)?;

        let mut engine: Option<DbServerEngine> = None;
        let mut version: Option<String> = None;
        let mut secret: Option<String> = None;

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "engine" => set_field(&mut engine, child, name)?,
                    "version" => set_field(&mut version, child, name)?,
                    "secret" => set_field(&mut secret, child, name)?,
                    _ => {
                        return Err(NodeError::UnknownField {
                            name: name.to_owned(),
                            span: child.span(),
                        });
                    }
                }
            }
        }

        Ok(DbServerConfig {
            engine: engine.ok_or(NodeError::MissingField {
                name: "engine",
                span: node.span(),
            })?,
            version: version.ok_or(NodeError::MissingField {
                name: "version",
                span: node.span(),
            })?,
            secret: secret.ok_or(NodeError::MissingField {
                name: "secret",
                span: node.span(),
            })?,
        })
    }
}

impl ValidateConfig for DbServerConfig {
    fn validate_config(&self) -> Result<(), String> {
        if self.version.trim().is_empty() {
            return Err("db version must not be empty".into());
        }

        if !secret_name(&self.secret) {
            return Err(format!("invalid secret name '{}'", self.secret));
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
        DbConfig::try_from(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_db_server(src: &str) -> Result<DbServerConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        DbServerConfig::try_from(doc.nodes().first().expect("test KDL must have a node"))
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
        assert_eq!(cfg.server, "pg-main");
        assert_eq!(cfg.user, "app1");
        assert_eq!(cfg.secret, "app1-pass");
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
        assert_eq!(cfg.secret, "pg-pass");
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
    fn parse_db_config() {
        let config: DbServerConfig = serde_yaml::from_str(
            r#"
engine: postgresql
version: "18"
secret: pg-pass
"#,
        )
        .unwrap();

        assert_eq!(config.engine, DbServerEngine::Postgresql);
        assert_eq!(config.version, "18");
        assert_eq!(config.secret, "pg-pass");
        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn reject_empty_version() {
        let config = DbServerConfig {
            engine: DbServerEngine::Postgresql,
            version: " ".into(),
            secret: "pg-pass".into(),
        };
        assert!(config.validate_config().is_err());
    }

    #[test]
    fn reject_invalid_secret_name() {
        let config = DbServerConfig {
            engine: DbServerEngine::Postgresql,
            version: "18".into(),
            secret: "Bad/Name".into(),
        };
        assert!(config.validate_config().is_err());
    }

    #[test]
    fn parse_db_unit_config() {
        let config: DbConfig = serde_yaml::from_str(
            r#"
server: pg-main
user: app1
secret: app1-pass
"#,
        )
        .unwrap();

        assert_eq!(config.server, "pg-main");
        assert_eq!(config.user, "app1");
        assert_eq!(config.secret, "app1-pass");
        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn db_config_rejects_invalid_fields() {
        let bad_server = DbConfig {
            server: "Bad/Name".into(),
            user: "app1".into(),
            secret: "app1-pass".into(),
        };
        assert!(bad_server.validate_config().is_err());

        let bad_user = DbConfig {
            server: "pg-main".into(),
            user: "Bad_User".into(),
            secret: "app1-pass".into(),
        };
        assert!(bad_user.validate_config().is_err());

        let bad_secret = DbConfig {
            server: "pg-main".into(),
            user: "app1".into(),
            secret: "Bad/Secret/".into(),
        };
        assert!(bad_secret.validate_config().is_err());
    }

    #[test]
    fn db_resolve_export_unknown_key() {
        let config = DbConfig {
            server: "pg-main".into(),
            user: "app1".into(),
            secret: "app1-pass".into(),
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
            server_dir.join("config.yaml"),
            "type: db-server\nengine: postgresql\nversion: \"18\"\nsecret: pg-pass\n",
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
            server: "pg-main".into(),
            user: "app1".into(),
            secret: "app1-pass".into(),
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
            server_dir.join("config.yaml"),
            "type: db-server\nengine: mariadb\nversion: \"12\"\nsecret: maria-pass\n",
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
            server: "maria-main".into(),
            user: "app1".into(),
            secret: "app1-pass".into(),
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
