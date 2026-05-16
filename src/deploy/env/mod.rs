mod value;

use std::collections::BTreeMap;

use kdl::KdlNode;
use serde::{
    Deserialize,
    Serialize,
};

pub use self::value::Value;
use crate::{
    MainContext,
    config::{
        ConfigNodeError,
        ValidateConfig,
    },
    error::{
        Location,
        RefError,
    },
    validate,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct EnvList(BTreeMap<String, Value>);

impl TryFrom<&KdlNode> for EnvList {
    type Error = ConfigNodeError;

    fn try_from(node: &KdlNode) -> Result<Self, Self::Error> {
        if let Some(stray) = node.entries().first() {
            return Err(ConfigNodeError::WrapperHasArgs { span: stray.span() });
        }

        let Some(children) = node.children() else {
            return Ok(Self::default());
        };

        let mut map = BTreeMap::new();
        for child in children.nodes() {
            let key = child.name().value().to_owned();
            let value = Value::try_from(child).map_err(|source| ConfigNodeError::InvalidField {
                name: key.clone(),
                span: child.span(),
                source,
            })?;
            map.insert(key, value);
        }

        Ok(Self(map))
    }
}

impl EnvList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve(
        &self,
        ctx: &MainContext,
        prefix: &str,
    ) -> Result<BTreeMap<String, String>, RefError> {
        self.0
            .iter()
            .map(|(k, v)| {
                let value = v
                    .render(ctx)
                    .map_err(|err| err.at(Location::field(format!("{prefix}.{k}"))))?;
                Ok((k.clone(), value))
            })
            .collect()
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
    use kdl::KdlDocument;

    use super::*;
    use crate::{
        config::{
            FieldError,
            TemplateError,
        },
        deploy::env::value::Ns,
    };

    fn parse_env(src: &str) -> Result<EnvList, ConfigNodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        let node = doc
            .nodes()
            .first()
            .expect("test KDL must have at least one node");
        EnvList::try_from(node)
    }

    #[test]
    fn kdl_empty_block() {
        let env = parse_env("env {}").unwrap();
        assert!(env.0.is_empty());
    }

    #[test]
    fn kdl_bare_node() {
        let env = parse_env("env").unwrap();
        assert!(env.0.is_empty());
    }

    #[test]
    fn kdl_string_value() {
        let env = parse_env(r#"env { DB_HOST "127.0.0.1" }"#).unwrap();
        let resolved = env.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(
            resolved.get("DB_HOST").map(String::as_str),
            Some("127.0.0.1"),
        );
    }

    #[test]
    fn kdl_integer_value() {
        let env = parse_env("env { DB_PORT 8000 }").unwrap();
        let resolved = env.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(resolved.get("DB_PORT").map(String::as_str), Some("8000"));
    }

    #[test]
    fn kdl_float_value() {
        let env = parse_env("env { RATIO 0.5 }").unwrap();
        let resolved = env.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(resolved.get("RATIO").map(String::as_str), Some("0.5"));
    }

    #[test]
    fn kdl_bool_value() {
        let env = parse_env("env { DEBUG #true }").unwrap();
        let resolved = env.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(resolved.get("DEBUG").map(String::as_str), Some("true"));
    }

    #[test]
    fn kdl_template_value() {
        let env = parse_env(r#"env { SECRET_KEY "${secret:nexus/secret-key}" }"#).unwrap();
        let v = env.0.get("SECRET_KEY").unwrap();
        let refs: Vec<(&Ns, &str)> = v.references().collect();
        assert_eq!(refs, vec![(&Ns::Secret, "nexus/secret-key")]);
    }

    #[test]
    fn kdl_multiple_entries_ordered() {
        let env = parse_env(
            r#"
            env {
                DB_HOST "127.0.0.1"
                DB_PORT 8000
            }
            "#,
        )
        .unwrap();
        let keys: Vec<&str> = env.0.keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["DB_HOST", "DB_PORT"]);
    }

    #[test]
    fn kdl_env_node_with_arg_rejected() {
        let err = parse_env(r#"env "stray" { DB_HOST "x" }"#).unwrap_err();
        assert!(
            matches!(&err, ConfigNodeError::WrapperHasArgs { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_env_node_with_property_rejected() {
        let err = parse_env(r#"env strict=#true { DB_HOST "x" }"#).unwrap_err();
        assert!(
            matches!(&err, ConfigNodeError::WrapperHasArgs { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_entry_error_carries_key_and_shape() {
        let err = parse_env("env { KEY }").unwrap_err();
        assert!(
            matches!(
                &err,
                ConfigNodeError::InvalidField {
                    name,
                    source: FieldError::EntryCount { .. },
                    ..
                } if name == "KEY",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_entry_error_carries_template_failure() {
        let err = parse_env(r#"env { BAD "${secret:}" }"#).unwrap_err();
        assert!(
            matches!(
                &err,
                ConfigNodeError::InvalidField {
                    name,
                    source: FieldError::InvalidTemplate {
                        source: TemplateError::MalformedRef { .. },
                        ..
                    },
                    ..
                } if name == "BAD",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn deserialize_unquoted_integer() {
        let list: EnvList = serde_yaml::from_str("DB_PORT: 5432").unwrap();
        let resolved = list.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(resolved.get("DB_PORT").map(String::as_str), Some("5432"));
    }

    #[test]
    fn deserialize_unquoted_bool_and_float() {
        let list: EnvList = serde_yaml::from_str("DEBUG: true\nRATIO: 0.5\n").unwrap();
        let resolved = list.resolve(&MainContext::default(), "env").unwrap();
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
}
