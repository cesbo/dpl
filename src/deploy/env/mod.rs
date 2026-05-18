mod value;

use std::collections::BTreeMap;

use kdl::{
    KdlDocument,
    KdlEntry,
    KdlNode,
};

pub use self::value::Value;
use crate::{
    MainContext,
    config::{
        FromKdlNode,
        NodeError,
        ValidateConfig,
    },
    error::{
        Location,
        RefError,
    },
    validate,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvList(BTreeMap<String, Value>);

impl FromKdlNode for EnvList {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        crate::kdl_args!(node)?;

        let Some(children) = node.children() else {
            return Ok(Self::default());
        };

        let mut map = BTreeMap::new();
        for child in children.nodes() {
            let key = child.name().value().to_owned();
            let value = Value::try_from(child).map_err(|source| NodeError::InvalidField {
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

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Render this env list back as a KDL node with the given name (typically
    /// `"env"`). Each entry becomes a child node `KEY "value"`, where the
    /// value is `Value::as_template()`.
    pub fn to_kdl_node(&self, name: &str) -> KdlNode {
        let mut node = KdlNode::new(name);
        let mut children = KdlDocument::new();
        for (k, v) in &self.0 {
            let mut entry_node = KdlNode::new(k.as_str());
            entry_node
                .entries_mut()
                .push(KdlEntry::new(v.as_template()));
            children.nodes_mut().push(entry_node);
        }
        node.set_children(children);
        node
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

    fn parse_env(src: &str) -> Result<EnvList, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        let node = doc
            .nodes()
            .first()
            .expect("test KDL must have at least one node");
        EnvList::from_kdl_node(node)
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
            matches!(&err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_env_node_with_property_rejected() {
        let err = parse_env(r#"env strict=#true { DB_HOST "x" }"#).unwrap_err();
        assert!(
            matches!(&err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_entry_error_carries_key_and_shape() {
        let err = parse_env("env { KEY }").unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
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
                NodeError::InvalidField {
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
    fn validate_config_rejects_invalid_env_name() {
        let list = parse_env(r#"env { BAD-NAME "x" }"#).unwrap();
        assert!(list.validate_config().is_err());
    }
}
