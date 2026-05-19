mod name;
mod value;

use std::collections::BTreeMap;

use kdl::{
    KdlDocument,
    KdlEntry,
    KdlNode,
};

pub use self::{
    name::Name,
    value::Value,
};
use crate::{
    MainContext,
    config::{
        FromKdlNode,
        NodeError,
    },
    error::{
        Location,
        RefError,
    },
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvList(BTreeMap<Name, Value>);

impl FromKdlNode for EnvList {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        crate::kdl_args!(node)?;

        let Some(children) = node.children() else {
            return Ok(Self::default());
        };

        let mut map = BTreeMap::new();
        for child in children.nodes() {
            let name = Name::from_kdl_node(child)?;
            let value = Value::from_kdl_node(child)?;
            map.insert(name, value);
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
                Ok((k.as_str().to_owned(), value))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use kdl::KdlDocument;

    use super::*;
    use crate::config::{
        FieldError,
        TemplateError,
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
    fn empty_block() {
        let env = parse_env("env {}").unwrap();
        assert!(env.0.is_empty());
    }

    #[test]
    fn bare_node() {
        let env = parse_env("env").unwrap();
        assert!(env.0.is_empty());
    }

    #[test]
    fn entries_sorted_and_resolved() {
        let env = parse_env(
            r#"
            env {
                DB_PORT 8000
                DB_HOST "127.0.0.1"
            }
            "#,
        )
        .unwrap();
        let resolved = env.resolve(&MainContext::default(), "env").unwrap();
        let pairs: Vec<(&str, &str)> = resolved
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(pairs, vec![("DB_HOST", "127.0.0.1"), ("DB_PORT", "8000")],);
    }

    #[test]
    fn rejects_arg() {
        let err = parse_env(r#"env "stray" { DB_HOST "x" }"#).unwrap_err();
        assert!(
            matches!(&err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn rejects_property() {
        let err = parse_env(r#"env strict=#true { DB_HOST "x" }"#).unwrap_err();
        assert!(
            matches!(&err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn error_carries_key() {
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
    fn invalid_env_name() {
        assert!(parse_env(r#"env { BAD-NAME "x" }"#).is_err());
    }

    #[test]
    fn wraps_template_error() {
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
}
