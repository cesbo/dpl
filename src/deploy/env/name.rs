use std::fmt;

use kdl::KdlNode;

use crate::config::{
    FieldError,
    FromKdlNode,
    NodeError,
};

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Name(String);

impl Name {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<Name> for String {
    fn from(name: Name) -> Self {
        name.0
    }
}

fn is_valid(name: &str) -> bool {
    if name.is_empty() {
        return false;
    }

    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return false;
    }

    name.as_bytes()
        .iter()
        .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
}

impl FromKdlNode for Name {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let ident = node.name();
        let raw = ident.value();
        if !is_valid(raw) {
            let span = ident.span();
            return Err(NodeError::InvalidField {
                name: raw.to_owned(),
                span,
                source: FieldError::InvalidValue {
                    expected: "env name (letters, digits, '_'; must not start with a digit)",
                    span,
                },
            });
        }
        Ok(Self(raw.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid() {
        assert!(is_valid("FOO"));
        assert!(is_valid("foo_bar"));
        assert!(is_valid("_private"));
        assert!(is_valid("X1"));
        assert!(is_valid("MIXED_Case_42"));
    }

    #[test]
    fn rejects_invalid() {
        assert!(!is_valid(""));
        assert!(!is_valid("1FOO"));
        assert!(!is_valid("FOO-BAR"));
        assert!(!is_valid("FOO BAR"));
        assert!(!is_valid("FOO.BAR"));
        assert!(!is_valid("ÜMLAUT"));
    }

    fn parse(src: &str) -> Result<Name, NodeError> {
        let doc: kdl::KdlDocument = src.parse().expect("test KDL must parse");
        let node = doc
            .nodes()
            .first()
            .expect("test KDL must have at least one node");
        Name::from_kdl_node(node)
    }

    #[test]
    fn from_node_valid() {
        let name = parse("FOO_BAR").unwrap();
        assert_eq!(name.as_str(), "FOO_BAR");
    }

    #[test]
    fn from_node_invalid_identifier() {
        let err = parse("BAD-NAME").unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "BAD-NAME",
            ),
            "unexpected error: {err:?}",
        );
    }
}
