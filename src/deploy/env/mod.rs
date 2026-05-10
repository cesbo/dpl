mod error;
mod value;

use std::collections::BTreeMap;

use serde::Deserialize;

pub use self::error::EnvError;
use self::value::Value;
use crate::{
    MainContext,
    config::ValidateConfig,
    validate,
};

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct EnvList(BTreeMap<String, Value>);

impl EnvList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve(&self, ctx: &MainContext) -> Result<BTreeMap<String, String>, EnvError> {
        self.0
            .iter()
            .map(|(k, v)| Ok((k.clone(), v.render(ctx)?)))
            .collect()
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), EnvError> {
        for value in self.0.values() {
            value.validate_references(ctx)?;
        }
        Ok(())
    }
}

impl ValidateConfig for EnvList {
    fn validate_config(&self) -> Result<(), String> {
        for key in self.0.keys() {
            if !validate::env_name(key) {
                return Err(format!("invalid env name: '{key}'"));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_literal_and_template() {
        let list: EnvList = serde_yaml::from_str(
            r#"
PORT: "8080"
DATABASE_URL: "postgres://app:${secret:db}@host/app"
TOKEN: "${secret:api-token}"
"#,
        )
        .unwrap();
        assert_eq!(list.0.len(), 3);
    }

    #[test]
    fn deserialize_unquoted_integer() {
        let list: EnvList = serde_yaml::from_str("DB_PORT: 5432").unwrap();
        let resolved = list.resolve(&MainContext::default()).unwrap();
        assert_eq!(resolved.get("DB_PORT").map(String::as_str), Some("5432"));
    }

    #[test]
    fn deserialize_unquoted_bool_and_float() {
        let list: EnvList = serde_yaml::from_str("DEBUG: true\nRATIO: 0.5\n").unwrap();
        let resolved = list.resolve(&MainContext::default()).unwrap();
        assert_eq!(resolved.get("DEBUG").map(String::as_str), Some("true"));
        assert_eq!(resolved.get("RATIO").map(String::as_str), Some("0.5"));
    }

    #[test]
    fn deserialize_rejects_template_syntax_error() {
        let err = serde_yaml::from_str::<EnvList>("BAD: \"${secret:}\"").unwrap_err();
        assert!(err.to_string().contains("malformed reference"));
    }

    #[test]
    fn validate_config_rejects_invalid_env_name() {
        let list: EnvList = serde_yaml::from_str("BAD-NAME: x").unwrap();
        assert!(list.validate_config().is_err());
    }

    #[test]
    fn validate_references_reports_missing_secret() {
        let list: EnvList = serde_yaml::from_str(r#"X: "${secret:nope}""#).unwrap();
        let err = list
            .validate_references(&MainContext::default())
            .unwrap_err();
        assert!(matches!(err, EnvError::MissingSecret { ref name } if name == "nope"));
    }

    #[test]
    fn resolve_unit_ref_to_db_user_and_name() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let dir = base.path().join("app-db");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.yaml"),
            "type: db\nserver: pg-main\nuser: app1\nsecret: app1-pass\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        let list: EnvList =
            serde_yaml::from_str(r#"DB_USER: "${app-db:user}"
DB_NAME: "${app-db:name}"
"#)
            .unwrap();
        list.validate_references(&ctx).unwrap();
        let resolved = list.resolve(&ctx).unwrap();
        assert_eq!(resolved.get("DB_USER").map(String::as_str), Some("app1"));
        assert_eq!(resolved.get("DB_NAME").map(String::as_str), Some("app-db"));
    }

    #[test]
    fn validate_references_reports_unknown_unit() {
        let list: EnvList = serde_yaml::from_str(r#"X: "${nope:user}""#).unwrap();
        let err = list
            .validate_references(&MainContext::default())
            .unwrap_err();
        assert!(matches!(err, EnvError::UnitNotFound { ref name } if name == "nope"));
    }

    #[test]
    fn validate_references_reports_unknown_export() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let dir = base.path().join("app-db");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("config.yaml"),
            "type: db\nserver: pg-main\nuser: app1\nsecret: app1-pass\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        let list: EnvList = serde_yaml::from_str(r#"X: "${app-db:port}""#).unwrap();
        let err = list.validate_references(&ctx).unwrap_err();
        let EnvError::UnknownExport { unit, kind, key } = err else {
            panic!("unexpected error variant");
        };
        assert_eq!(unit, "app-db");
        assert_eq!(kind, "db");
        assert_eq!(key, "port");
    }

    #[test]
    fn validate_references_rejects_unit_without_exports() {
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

        let list: EnvList = serde_yaml::from_str(r#"X: "${example-com:host}""#).unwrap();
        let err = list.validate_references(&ctx).unwrap_err();
        assert!(matches!(
            err,
            EnvError::UnknownExport { kind, .. } if kind == "domain"
        ));
    }
}
