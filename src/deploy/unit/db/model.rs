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
    config::ValidateConfig,
    deploy::unit::UnitConfig,
    validate::{
        resource_name,
        secret_name,
    },
};

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
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), String> {
        if !ctx.secret_exists(&self.secret) {
            return Err(format!("secret '{}' not found", self.secret));
        }

        match UnitConfig::load(ctx, &self.server) {
            Ok(UnitConfig::DbServer(_)) => {}
            Ok(_) => {
                return Err(format!("server '{}' is not a db-server", self.server));
            }
            Err(err) => {
                return Err(format!("load server '{}': {err}", self.server));
            }
        }

        Ok(())
    }

    pub fn has_export(key: &str) -> bool {
        matches!(key, "user" | "name" | "password" | "host" | "port" | "url")
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        name: &str,
        key: &str,
    ) -> Result<String, String> {
        match key {
            "user" => Ok(self.user.clone()),
            "name" => Ok(name.to_owned()),
            "password" => ctx
                .resolve_secret(&self.secret)
                .map_err(|err| format!("decrypt secret '{}': {err}", self.secret)),
            "host" => Ok(self.server.clone()),
            "port" => {
                let server = self.load_server(ctx)?;
                Ok(server.engine.default_port().to_string())
            }
            "url" => {
                let server = self.load_server(ctx)?;
                let password = ctx
                    .resolve_secret(&self.secret)
                    .map_err(|err| format!("decrypt secret '{}': {err}", self.secret))?;
                Ok(format!(
                    "{scheme}://{user}:{password}@{host}:{port}/{db}",
                    scheme = server.engine.url_scheme(),
                    user = self.user,
                    password = userinfo_encode(&password),
                    host = self.server,
                    port = server.engine.default_port(),
                    db = name,
                ))
            }
            _ => Err(format!("unknown export '{key}'")),
        }
    }

    fn load_server(&self, ctx: &MainContext) -> Result<DbServerConfig, String> {
        let server = UnitConfig::load(ctx, &self.server)
            .map_err(|err| format!("load server '{}': {err}", self.server))?;
        let UnitConfig::DbServer(server) = server else {
            return Err(format!("'{}' is not a db-server", self.server));
        };
        Ok(server)
    }
}

/// RFC 3986 *unreserved* set: encode everything except `A-Z a-z 0-9 - . _ ~`.
const USERINFO: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

fn userinfo_encode(s: &str) -> String {
    utf8_percent_encode(s, USERINFO).to_string()
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

impl DbServerConfig {
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), String> {
        if !ctx.secret_exists(&self.secret) {
            return Err(format!("secret '{}' not found", self.secret));
        }
        Ok(())
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
    use super::*;

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
    fn db_has_export() {
        assert!(DbConfig::has_export("user"));
        assert!(DbConfig::has_export("name"));
        assert!(DbConfig::has_export("password"));
        assert!(DbConfig::has_export("host"));
        assert!(DbConfig::has_export("port"));
        assert!(!DbConfig::has_export("unknown"));
        assert!(!DbConfig::has_export(""));
    }

    #[test]
    fn db_resolve_export_user_and_name() {
        let config = DbConfig {
            server: "pg-main".into(),
            user: "app1".into(),
            secret: "app1-pass".into(),
        };
        let ctx = MainContext::default();

        assert_eq!(
            config.resolve_export(&ctx, "app-db", "user").unwrap(),
            "app1"
        );
        assert_eq!(
            config.resolve_export(&ctx, "app-db", "name").unwrap(),
            "app-db"
        );
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
        assert_eq!(err, "unknown export 'unknown'");
    }

    #[test]
    fn db_resolve_export_host() {
        let config = DbConfig {
            server: "pg-main".into(),
            user: "app1".into(),
            secret: "app1-pass".into(),
        };
        assert_eq!(
            config
                .resolve_export(&MainContext::default(), "app-db", "host")
                .unwrap(),
            "pg-main"
        );
    }

    #[test]
    fn db_resolve_export_port_postgres() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let server_dir = base.path().join("pg-main");
        fs::create_dir_all(&server_dir).unwrap();
        fs::write(
            server_dir.join("config.yaml"),
            "type: db-server\nengine: postgresql\nversion: \"18\"\nsecret: pg-pass\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config = DbConfig {
            server: "pg-main".into(),
            user: "app1".into(),
            secret: "app1-pass".into(),
        };
        assert_eq!(
            config.resolve_export(&ctx, "app-db", "port").unwrap(),
            "5432"
        );
    }

    #[test]
    fn db_resolve_export_port_mariadb() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let server_dir = base.path().join("maria-main");
        fs::create_dir_all(&server_dir).unwrap();
        fs::write(
            server_dir.join("config.yaml"),
            "type: db-server\nengine: mariadb\nversion: \"12\"\nsecret: maria-pass\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config = DbConfig {
            server: "maria-main".into(),
            user: "app1".into(),
            secret: "app1-pass".into(),
        };
        assert_eq!(
            config.resolve_export(&ctx, "app-db", "port").unwrap(),
            "3306"
        );
    }

    #[test]
    fn db_resolve_export_password_requires_secret() {
        use tempfile::TempDir;

        use crate::secret::MasterKey;

        let base = TempDir::new().unwrap();
        let key = MasterKey::generate(base.path());
        key.save().unwrap();
        key.encrypt_to_file("app1-pass", "topsecret").unwrap();

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
            config.resolve_export(&ctx, "app-db", "password").unwrap(),
            "topsecret"
        );
    }

    #[test]
    fn db_has_export_url() {
        assert!(DbConfig::has_export("url"));
    }

    #[test]
    fn db_resolve_export_url_postgres() {
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
        key.encrypt_to_file("app1-pass", "topsecret").unwrap();

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
            config.resolve_export(&ctx, "app-db", "url").unwrap(),
            "postgresql://app1:topsecret@pg-main:5432/app-db"
        );
    }

    #[test]
    fn db_resolve_export_url_mariadb() {
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
        key.encrypt_to_file("app1-pass", "topsecret").unwrap();

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
            config.resolve_export(&ctx, "app-db", "url").unwrap(),
            "mysql://app1:topsecret@maria-main:3306/app-db"
        );
    }

    #[test]
    fn db_resolve_export_url_encodes_password() {
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
        key.encrypt_to_file("app1-pass", "p@ss:w/rd#1").unwrap();

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
            config.resolve_export(&ctx, "app-db", "url").unwrap(),
            "postgresql://app1:p%40ss%3Aw%2Frd%231@pg-main:5432/app-db"
        );
    }

    #[test]
    fn userinfo_encode_basics() {
        assert_eq!(userinfo_encode("abcXYZ012"), "abcXYZ012");
        assert_eq!(userinfo_encode("-._~"), "-._~");
        assert_eq!(userinfo_encode(" "), "%20");
        assert_eq!(userinfo_encode("@"), "%40");
        assert_eq!(userinfo_encode(":"), "%3A");
        assert_eq!(userinfo_encode("/"), "%2F");
        assert_eq!(userinfo_encode("%"), "%25");
        assert_eq!(userinfo_encode("#"), "%23");
        // Non-ASCII byte (UTF-8 'ñ' = 0xC3 0xB1) encodes byte-by-byte.
        assert_eq!(userinfo_encode("ñ"), "%C3%B1");
    }

    #[test]
    fn engine_metadata() {
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
