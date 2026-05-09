use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    config::ValidateConfig,
    validate::secret_name,
};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DbConfig {
    pub engine: DbEngine,
    pub version: String,
    pub secret: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DbEngine {
    Postgresql,
    Mariadb,
}

impl DbEngine {
    pub fn as_str(&self) -> &'static str {
        match self {
            DbEngine::Postgresql => "postgresql",
            DbEngine::Mariadb => "mariadb",
        }
    }

    pub fn image(&self, version: &str) -> String {
        match self {
            DbEngine::Postgresql => format!("docker.io/library/postgres:{version}"),
            DbEngine::Mariadb => format!("docker.io/library/mariadb:{version}"),
        }
    }

    pub fn data_path(&self) -> &'static str {
        match self {
            DbEngine::Postgresql => "/var/lib/postgresql",
            DbEngine::Mariadb => "/var/lib/mysql",
        }
    }

    pub fn password_env(&self) -> &'static str {
        match self {
            DbEngine::Postgresql => "POSTGRES_PASSWORD",
            DbEngine::Mariadb => "MARIADB_ROOT_PASSWORD",
        }
    }

    pub fn default_version(&self) -> &'static str {
        match self {
            DbEngine::Postgresql => "18-alpine",
            DbEngine::Mariadb => "12",
        }
    }
}

impl ValidateConfig for DbConfig {
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
        let config: DbConfig = serde_yaml::from_str(
            r#"
engine: postgresql
version: "18"
secret: pg-pass
"#,
        )
        .unwrap();

        assert_eq!(config.engine, DbEngine::Postgresql);
        assert_eq!(config.version, "18");
        assert_eq!(config.secret, "pg-pass");
        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn reject_empty_version() {
        let config = DbConfig {
            engine: DbEngine::Postgresql,
            version: " ".into(),
            secret: "pg-pass".into(),
        };
        assert!(config.validate_config().is_err());
    }

    #[test]
    fn reject_invalid_secret_name() {
        let config = DbConfig {
            engine: DbEngine::Postgresql,
            version: "18".into(),
            secret: "Bad/Name".into(),
        };
        assert!(config.validate_config().is_err());
    }

    #[test]
    fn engine_metadata() {
        let engine = DbEngine::Postgresql;
        assert_eq!(engine.as_str(), "postgresql");
        assert_eq!(engine.image("18-alpine"), "docker.io/library/postgres:18-alpine");
        assert_eq!(engine.data_path(), "/var/lib/postgresql");
        assert_eq!(engine.password_env(), "POSTGRES_PASSWORD");
        assert_eq!(engine.default_version(), "18-alpine");

        let engine = DbEngine::Mariadb;
        assert_eq!(engine.as_str(), "mariadb");
        assert_eq!(engine.image("12"), "docker.io/library/mariadb:12");
        assert_eq!(engine.data_path(), "/var/lib/mysql");
        assert_eq!(engine.password_env(), "MARIADB_ROOT_PASSWORD");
        assert_eq!(engine.default_version(), "12");
    }
}
