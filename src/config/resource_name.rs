use std::{
    fmt,
    path::PathBuf,
};

use kdl::{
    KdlEntry,
    KdlNode,
    KdlValue,
};
use serde::{
    Serialize,
    Serializer,
};
use thiserror::Error;

use super::{
    FieldError,
    FromKdlArg,
    FromKdlNode,
    NodeError,
    parse_string_child,
};
use crate::MainContext;

const EXPECTED: &str =
    "resource name (lowercase a-z, digits, '-'; not starting/ending with '-'; no '--')";

#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[error("invalid resource name '{input}' (expected: {EXPECTED})")]
pub struct InvalidResourceName {
    pub input: String,
}

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceName(String);

impl ResourceName {
    pub fn new(raw: &str) -> Result<Self, InvalidResourceName> {
        if !Self::is_valid(raw) {
            return Err(InvalidResourceName {
                input: raw.to_owned(),
            });
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        if name.starts_with('-') || name.ends_with('-') {
            return false;
        }

        if name.contains("--") {
            return false;
        }

        name.as_bytes()
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    pub fn unit_dir(&self, ctx: &MainContext) -> PathBuf {
        ctx.base().join(self.as_str())
    }
}

impl fmt::Display for ResourceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for ResourceName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl From<&ResourceName> for KdlEntry {
    fn from(value: &ResourceName) -> Self {
        KdlEntry::new(String::from(value.as_str()))
    }
}

impl FromKdlNode for ResourceName {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let raw =
            parse_string_child(node).map_err(|source| NodeError::invalid_field(node, source))?;

        Self::new(raw).map_err(|_| {
            let span = node.entries()[0].span();
            NodeError::InvalidField {
                name: node.name().value().to_owned(),
                span,
                source: FieldError::InvalidValue {
                    expected: EXPECTED,
                    span,
                },
            }
        })
    }
}

impl FromKdlArg for ResourceName {
    fn from_kdl_arg(entry: &KdlEntry, field: &str) -> Result<Self, NodeError> {
        if entry.name().is_some() {
            return Err(NodeError::InvalidField {
                name: field.to_owned(),
                span: entry.span(),
                source: FieldError::NamedEntry { span: entry.span() },
            });
        }

        let KdlValue::String(s) = entry.value() else {
            return Err(NodeError::InvalidField {
                name: field.to_owned(),
                span: entry.span(),
                source: FieldError::InvalidType {
                    expected: "string",
                    span: entry.span(),
                },
            });
        };

        Self::new(s).map_err(|_| NodeError::InvalidField {
            name: field.to_owned(),
            span: entry.span(),
            source: FieldError::InvalidValue {
                expected: EXPECTED,
                span: entry.span(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use kdl::KdlDocument;

    use super::*;
    use crate::kdl_args;

    fn node(src: &str) -> KdlNode {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        doc.nodes()
            .first()
            .expect("test KDL must have one node")
            .clone()
    }

    #[test]
    fn new_accepts_valid() {
        assert_eq!(ResourceName::new("pg-main").unwrap().as_str(), "pg-main");
        assert_eq!(ResourceName::new("a").unwrap().as_str(), "a");
        assert_eq!(ResourceName::new("a-b-c").unwrap().as_str(), "a-b-c");
        assert_eq!(ResourceName::new("pg1").unwrap().as_str(), "pg1");
    }

    #[test]
    fn new_rejects_invalid() {
        for bad in [
            "", ".", "foo/bar", "foo_bar", "foo.bar", "foo--bar", " foo", "Ümlaut", "FOO", "-foo",
            "foo-",
        ] {
            let err = ResourceName::new(bad).unwrap_err();
            assert_eq!(err.input, bad, "expected error to carry input {bad:?}");
        }
    }

    #[test]
    fn from_node_happy() {
        let n = node(r#"server "pg-main""#);
        let name = ResourceName::from_kdl_node(&n).unwrap();
        assert_eq!(name.as_str(), "pg-main");
    }

    #[test]
    fn from_node_rejects_invalid_value() {
        let n = node(r#"server "Bad/Name""#);
        let err = ResourceName::from_kdl_node(&n).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "server",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn from_node_rejects_wrong_type() {
        let n = node("server 5");
        let err = ResourceName::from_kdl_node(&n).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    source: FieldError::InvalidType {
                        expected: "string",
                        ..
                    },
                    ..
                },
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn from_arg_happy() {
        let n = node(r#"timer "run-tasks""#);
        let name: ResourceName = kdl_args!(&n, name: ResourceName).unwrap();
        assert_eq!(name.as_str(), "run-tasks");
    }

    #[test]
    fn from_arg_rejects_invalid_value() {
        let n = node(r#"timer "Bad_Name""#);
        let err = kdl_args!(&n, name: ResourceName).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "name",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn from_arg_rejects_named_entry() {
        let n = node(r#"timer name="run-tasks""#);
        let err = kdl_args!(&n, name: ResourceName).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    source: FieldError::NamedEntry { .. },
                    ..
                },
            ),
            "unexpected: {err:?}",
        );
    }
}
