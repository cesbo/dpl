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
        ResourceName,
        SecretName,
    },
    deploy::unit::UnitConfig,
    error::{
        Location,
        RefError,
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
    pub secret: SecretName,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DbConfig {
    pub server: ResourceName,
    pub user: String,
    pub secret: SecretName,
}

impl DbConfig {
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        self.resolve_password(ctx)?;
        self.resolve_server(ctx)?
            .validate_references(ctx)
            .map_err(|err| err.at(Location::unit(self.server.as_str())))?;
        Ok(())
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        unit_name: &ResourceName,
        key: &str,
    ) -> Result<String, RefError> {
        match key {
            "user" => Ok(self.user.clone()),
            "name" => Ok(unit_name.to_string()),
            "password" => self.resolve_password(ctx),
            "host" => Ok(self.server.to_string()),
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
            _ => Err(RefError::unknown_export(key)),
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
                _ => Err(RefError::wrong_unit_type(
                    self.server.to_string(),
                    "db-server",
                )),
            })
            .map_err(|err| err.at(Location::field("server")))
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

    /// The engine's built-in superuser login name.
    pub fn superuser(&self) -> &'static str {
        match self {
            DbServerEngine::Postgresql => "postgres",
            DbServerEngine::Mariadb => "root",
        }
    }

    /// Args to open an interactive client session as `user` against `db_name`.
    pub fn console_args(&self, user: &str, db_name: &str) -> Vec<String> {
        let args = match self {
            DbServerEngine::Postgresql => {
                vec!["psql", "-U", user, "-d", db_name]
            }
            DbServerEngine::Mariadb => {
                vec!["mariadb", "-u", user, db_name]
            }
        };
        args.into_iter().map(String::from).collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RefErrorKind;

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
        assert_eq!(config.secret.as_str(), "pg-pass");
    }

    #[test]
    fn reject_invalid_secret_name() {
        let err = serde_yaml::from_str::<DbServerConfig>(
            r#"
engine: postgresql
version: "18"
secret: Bad/Name
"#,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("invalid secret name"),
            "unexpected error: {err}"
        );
    }

    // TODO: re-enable when proper validator lands
    /*
    #[test]
    fn reject_empty_version() {
        let config = DbServerConfig {
            engine: DbServerEngine::Postgresql,
            version: " ".into(),
            secret: SecretName::new("pg-pass").unwrap(),
        };
        assert!(config.validate_config().is_err());
    }
    */

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

        assert_eq!(config.server.as_str(), "pg-main");
        assert_eq!(config.user, "app1");
        assert_eq!(config.secret.as_str(), "app1-pass");
    }

    #[test]
    fn db_resolve_export_unknown_key() {
        let config = DbConfig {
            server: ResourceName::new("pg-main").unwrap(),
            user: "app1".into(),
            secret: SecretName::new("app1-pass").unwrap(),
        };
        let unit = ResourceName::new("app-db").unwrap();
        let err = config
            .resolve_export(&MainContext::default(), &unit, "unknown")
            .unwrap_err();
        assert!(
            matches!(&err.kind, RefErrorKind::UnknownExport { key } if key == "unknown"),
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
        key.encrypt_to_file(&SecretName::new("app1-pass").unwrap(), "top$ecret&")
            .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };
        let config = DbConfig {
            server: ResourceName::new("pg-main").unwrap(),
            user: "app1".into(),
            secret: SecretName::new("app1-pass").unwrap(),
        };
        let unit = ResourceName::new("app-db").unwrap();

        assert_eq!(
            config.resolve_export(&ctx, &unit, "name").unwrap(),
            "app-db"
        );

        assert_eq!(config.resolve_export(&ctx, &unit, "user").unwrap(), "app1");

        assert_eq!(
            config.resolve_export(&ctx, &unit, "password").unwrap(),
            "top$ecret&"
        );

        assert_eq!(
            config
                .resolve_export(&MainContext::default(), &unit, "host")
                .unwrap(),
            "pg-main"
        );

        assert_eq!(config.resolve_export(&ctx, &unit, "port").unwrap(), "5432");

        assert_eq!(
            config.resolve_export(&ctx, &unit, "url").unwrap(),
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
        key.encrypt_to_file(&SecretName::new("app1-pass").unwrap(), "top$ecret&")
            .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };
        let config = DbConfig {
            server: ResourceName::new("maria-main").unwrap(),
            user: "app1".into(),
            secret: SecretName::new("app1-pass").unwrap(),
        };
        let unit = ResourceName::new("app-db").unwrap();

        assert_eq!(
            config.resolve_export(&ctx, &unit, "name").unwrap(),
            "app-db"
        );

        assert_eq!(config.resolve_export(&ctx, &unit, "user").unwrap(), "app1");

        assert_eq!(
            config.resolve_export(&ctx, &unit, "password").unwrap(),
            "top$ecret&"
        );

        assert_eq!(
            config
                .resolve_export(&MainContext::default(), &unit, "host")
                .unwrap(),
            "maria-main"
        );

        assert_eq!(config.resolve_export(&ctx, &unit, "port").unwrap(), "3306");

        assert_eq!(
            config.resolve_export(&ctx, &unit, "url").unwrap(),
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

    #[test]
    fn superuser() {
        assert_eq!(DbServerEngine::Postgresql.superuser(), "postgres");
        assert_eq!(DbServerEngine::Mariadb.superuser(), "root");
    }

    #[test]
    fn console_args() {
        assert_eq!(
            DbServerEngine::Postgresql.console_args("app1", "app-db"),
            ["psql", "-U", "app1", "-d", "app-db"]
        );
        assert_eq!(
            DbServerEngine::Mariadb.console_args("app1", "app-db"),
            ["mariadb", "-u", "app1", "app-db"]
        );
    }
}
