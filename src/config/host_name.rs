use std::fmt;

use kdl::{
    KdlEntry,
    KdlNode,
};
use serde::{
    Serialize,
    Serializer,
};
use thiserror::Error;

use super::{
    FieldError,
    FromKdlNode,
    NodeError,
    ResourceName,
    parse_string_child,
};

const EXPECTED: &str = "host name (dot-separated lowercase a-z/digit/'-' labels; optional leading '*.' wildcard requires at least two further labels)";

#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[error("invalid host name '{input}' (expected: {EXPECTED})")]
pub struct InvalidHostName {
    pub input: String,
}

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostName(String);

impl HostName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn new(raw: &str) -> Result<Self, InvalidHostName> {
        if !Self::is_valid(raw) {
            return Err(InvalidHostName {
                input: raw.to_owned(),
            });
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn is_valid(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        let mut labels = name.split('.');
        let first = labels.next().expect("split yields at least one item");

        if first == "*" {
            let rest: Vec<&str> = labels.collect();
            if rest.len() < 2 {
                return false;
            }
            rest.iter().all(|label| ResourceName::is_valid(label))
        } else {
            ResourceName::is_valid(first) && labels.all(ResourceName::is_valid)
        }
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for HostName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl From<&HostName> for KdlEntry {
    fn from(value: &HostName) -> Self {
        KdlEntry::new(String::from(value.as_str()))
    }
}

impl FromKdlNode for HostName {
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

#[cfg(test)]
mod tests {
    use kdl::KdlDocument;

    use super::*;

    fn node(src: &str) -> KdlNode {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        doc.nodes()
            .first()
            .expect("test KDL must have one node")
            .clone()
    }

    #[test]
    fn new_accepts_valid() {
        for ok in [
            "example.com",
            "www.example.com",
            "localhost",
            "*.example.com",
            "*.api.example.com",
            "a-b.c-d.example.com",
            "pg1.internal",
        ] {
            assert_eq!(HostName::new(ok).unwrap().as_str(), ok);
        }
    }

    #[test]
    fn new_rejects_invalid() {
        for bad in [
            "",
            "Example.com",
            ".example.com",
            "example.com.",
            "example..com",
            "-foo.com",
            "foo-.com",
            "foo_bar.com",
            "*.com",
            "foo.*.com",
            "**.example.com",
            "*example.com",
            "*",
        ] {
            let err = HostName::new(bad).unwrap_err();
            assert_eq!(err.input, bad, "expected error to carry input {bad:?}");
        }
    }

    #[test]
    fn from_node_happy() {
        let n = node(r#"host "example.com""#);
        let name = HostName::from_kdl_node(&n).unwrap();
        assert_eq!(name.as_str(), "example.com");
    }

    #[test]
    fn from_node_happy_wildcard() {
        let n = node(r#"host "*.example.com""#);
        let name = HostName::from_kdl_node(&n).unwrap();
        assert_eq!(name.as_str(), "*.example.com");
    }

    #[test]
    fn from_node_rejects_invalid_value() {
        let n = node(r#"host "Bad_Host""#);
        let err = HostName::from_kdl_node(&n).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "host",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn from_node_rejects_empty() {
        let n = node(r#"host """#);
        let err = HostName::from_kdl_node(&n).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    source: FieldError::InvalidValue { .. },
                    ..
                },
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn from_node_rejects_wrong_type() {
        let n = node("host 5");
        let err = HostName::from_kdl_node(&n).unwrap_err();
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
    fn from_node_rejects_named_entry() {
        let n = node(r#"host value="example.com""#);
        let err = HostName::from_kdl_node(&n).unwrap_err();
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
