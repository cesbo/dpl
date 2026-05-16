mod value;

use std::collections::BTreeMap;

use kdl::{
    KdlNode,
    KdlValue,
};
use miette::SourceSpan;
use serde::{
    Deserialize,
    Serialize,
};
use thiserror::Error;

pub use self::value::{
    Value,
    ValueError,
};
use crate::{
    MainContext,
    config::ValidateConfig,
    error::{
        Location,
        RefError,
    },
    validate,
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct EnvList(BTreeMap<String, Value>);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum EnvListError {
    #[error("env block must not have arguments or properties")]
    EnvNodeHasArgs { span: SourceSpan },

    #[error("invalid syntax for env var '{key}'")]
    InvalidSyntax { key: String, span: SourceSpan },

    #[error("invalid template in env var '{key}': {source}")]
    InvalidTemplate {
        key: String,
        span: SourceSpan,
        #[source]
        source: ValueError,
    },
}

impl TryFrom<&KdlNode> for EnvList {
    type Error = EnvListError;

    fn try_from(node: &KdlNode) -> Result<Self, Self::Error> {
        if let Some(stray) = node.entries().first() {
            return Err(EnvListError::EnvNodeHasArgs { span: stray.span() });
        }

        let Some(children) = node.children() else {
            return Ok(Self::default());
        };

        let mut map = BTreeMap::new();
        for child in children.nodes() {
            let key = child.name().value().to_owned();

            if child.children().is_some() {
                return Err(EnvListError::InvalidSyntax {
                    key,
                    span: child.span(),
                });
            }

            let entries = child.entries();
            for entry in entries {
                if entry.name().is_some() {
                    return Err(EnvListError::InvalidSyntax {
                        key,
                        span: entry.span(),
                    });
                }
            }

            if entries.len() != 1 {
                return Err(EnvListError::InvalidSyntax {
                    key,
                    span: child.span(),
                });
            }

            let entry = &entries[0];
            let value = match entry.value() {
                KdlValue::String(s) => {
                    Value::parse(s).map_err(|source| EnvListError::InvalidTemplate {
                        key: key.clone(),
                        span: entry.span(),
                        source,
                    })?
                }
                KdlValue::Integer(i) => Value::parse(&i.to_string())
                    .expect("integer literal cannot contain template syntax"),
                KdlValue::Float(f) => Value::parse(&f.to_string())
                    .expect("float literal cannot contain template syntax"),
                KdlValue::Bool(b) => Value::parse(if *b { "true" } else { "false" })
                    .expect("bool literal cannot contain template syntax"),
                KdlValue::Null => {
                    return Err(EnvListError::InvalidSyntax {
                        key,
                        span: entry.span(),
                    });
                }
            };

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
    use crate::deploy::env::value::Ns;

    fn parse_env(src: &str) -> Result<EnvList, EnvListError> {
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
            matches!(&err, EnvListError::EnvNodeHasArgs { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_env_node_with_property_rejected() {
        let err = parse_env(r#"env strict=#true { DB_HOST "x" }"#).unwrap_err();
        assert!(
            matches!(&err, EnvListError::EnvNodeHasArgs { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_missing_value_rejected() {
        let err = parse_env("env { KEY }").unwrap_err();
        assert!(
            matches!(&err, EnvListError::InvalidSyntax { key, .. } if key == "KEY"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_too_many_values_rejected() {
        let err = parse_env(r#"env { KEY "a" "b" }"#).unwrap_err();
        assert!(
            matches!(&err, EnvListError::InvalidSyntax { key, .. } if key == "KEY"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_named_entry_rejected() {
        let err = parse_env(r#"env { KEY value="x" }"#).unwrap_err();
        assert!(
            matches!(&err, EnvListError::InvalidSyntax { key, .. } if key == "KEY"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_child_block_rejected() {
        let err = parse_env(r#"env { KEY "x" { extra } }"#).unwrap_err();
        assert!(
            matches!(&err, EnvListError::InvalidSyntax { key, .. } if key == "KEY"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_null_value_rejected() {
        let err = parse_env("env { KEY #null }").unwrap_err();
        assert!(
            matches!(&err, EnvListError::InvalidSyntax { key, .. } if key == "KEY"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_invalid_template_rejected() {
        let err = parse_env(r#"env { BAD "${secret:}" }"#).unwrap_err();
        assert!(
            matches!(
                &err,
                EnvListError::InvalidTemplate {
                    key,
                    source: ValueError::MalformedRef { .. },
                    ..
                } if key == "BAD",
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
