use std::fmt;

use kdl::{
    KdlEntry,
    KdlNode,
    KdlValue,
};
use serde::{
    Serialize,
    Serializer,
};

use super::{
    FieldError,
    FromKdlArg,
    FromKdlNode,
    NodeError,
    parse_string_child,
};
use crate::validate;

const EXPECTED: &str =
    "resource name (lowercase a-z, digits, '-'; not starting/ending with '-'; no '--')";

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceName(String);

impl ResourceName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn new(raw: &str) -> Self {
        Self(raw.to_owned())
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

        if !validate::resource_name(raw) {
            let span = node.entries()[0].span();
            return Err(NodeError::InvalidField {
                name: node.name().value().to_owned(),
                span,
                source: FieldError::InvalidValue {
                    expected: EXPECTED,
                    span,
                },
            });
        }

        Ok(Self(raw.to_owned()))
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

        if !validate::resource_name(s) {
            return Err(NodeError::InvalidField {
                name: field.to_owned(),
                span: entry.span(),
                source: FieldError::InvalidValue {
                    expected: EXPECTED,
                    span: entry.span(),
                },
            });
        }

        Ok(Self(s.clone()))
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
