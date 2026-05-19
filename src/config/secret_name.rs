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
    parse_string_child,
};
use crate::validate;

const EXPECTED: &str = "secret name (one or more '/'-separated lowercase a-z, digits, '-' segments; no leading/trailing '-'; no '--')";

#[derive(Error, Debug, Clone, PartialEq, Eq)]
#[error("invalid secret name '{input}' (expected: {EXPECTED})")]
pub struct InvalidSecretName {
    pub input: String,
}

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretName(String);

impl SecretName {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn new(raw: &str) -> Result<Self, InvalidSecretName> {
        if !validate::secret_name(raw) {
            return Err(InvalidSecretName {
                input: raw.to_owned(),
            });
        }
        Ok(Self(raw.to_owned()))
    }
}

impl fmt::Display for SecretName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for SecretName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl From<&SecretName> for KdlEntry {
    fn from(value: &SecretName) -> Self {
        KdlEntry::new(String::from(value.as_str()))
    }
}

impl FromKdlNode for SecretName {
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
        assert_eq!(SecretName::new("app1-pass").unwrap().as_str(), "app1-pass");
        assert_eq!(
            SecretName::new("db/prod-password").unwrap().as_str(),
            "db/prod-password",
        );
    }

    #[test]
    fn new_rejects_invalid() {
        for bad in ["", "Bad/Name/", "foo//bar", "FOO", "-foo"] {
            let err = SecretName::new(bad).unwrap_err();
            assert_eq!(err.input, bad, "expected error to carry input {bad:?}");
        }
    }

    #[test]
    fn from_node_happy_simple() {
        let n = node(r#"secret "app1-pass""#);
        let name = SecretName::from_kdl_node(&n).unwrap();
        assert_eq!(name.as_str(), "app1-pass");
    }

    #[test]
    fn from_node_happy_path() {
        let n = node(r#"secret "db/prod-password""#);
        let name = SecretName::from_kdl_node(&n).unwrap();
        assert_eq!(name.as_str(), "db/prod-password");
    }

    #[test]
    fn from_node_rejects_invalid_value() {
        let n = node(r#"secret "Bad/Name/""#);
        let err = SecretName::from_kdl_node(&n).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "secret",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn from_node_rejects_empty() {
        let n = node(r#"secret """#);
        let err = SecretName::from_kdl_node(&n).unwrap_err();
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
        let n = node("secret 5");
        let err = SecretName::from_kdl_node(&n).unwrap_err();
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
}
