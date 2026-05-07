mod error;
mod value;

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{
    MainContext,
    config::ValidateConfig,
    validate,
};

pub use self::error::EnvError;
use self::value::Value;

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
            value.validate(ctx)?;
        }
        Ok(())
    }
}

#[cfg(test)]
impl EnvList {
    pub fn insert_literal(&mut self, key: String, value: String) {
        self.0
            .insert(key, Value::parse(&value).expect("literal must not contain '$'"));
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

    fn list_from_yaml(yaml: &str) -> EnvList {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn deserialize_literal_and_template() {
        let list = list_from_yaml(
            r#"
PORT: "8080"
DATABASE_URL: "postgres://app:${secret:db}@host/app"
TOKEN: "${secret:api-token}"
"#,
        );
        assert_eq!(list.0.len(), 3);
    }

    #[test]
    fn deserialize_unquoted_integer() {
        let list = list_from_yaml("DB_PORT: 5432\n");
        let resolved = list.resolve(&MainContext::default()).unwrap();
        assert_eq!(resolved.get("DB_PORT").map(String::as_str), Some("5432"));
    }

    #[test]
    fn deserialize_unquoted_bool_and_float() {
        let list = list_from_yaml("DEBUG: true\nRATIO: 0.5\n");
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
        let mut list = EnvList::new();
        list.insert_literal("BAD-NAME".into(), "x".into());
        assert!(list.validate_config().is_err());
    }

    #[test]
    fn validate_references_reports_missing_secret() {
        let list = list_from_yaml(r#"X: "${secret:nope}""#);
        let err = list.validate_references(&MainContext::default()).unwrap_err();
        assert!(matches!(err, EnvError::MissingSecret { ref name } if name == "nope"));
    }
}
