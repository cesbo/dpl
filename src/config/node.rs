use std::ops::RangeInclusive;

use kdl::{
    KdlEntry,
    KdlNode,
    KdlValue,
};

use super::{
    FieldError,
    NodeError,
};

/// Build a child node shaped like `name "value"`.
pub fn string_node(name: &str, value: &str) -> KdlNode {
    let mut node = KdlNode::new(name);
    node.entries_mut().push(KdlEntry::new(value.to_owned()));
    node
}

/// Build a child node shaped like `name 8000`.
pub fn integer_node(name: &str, value: impl Into<i128>) -> KdlNode {
    let mut node = KdlNode::new(name);
    let value = value.into();
    node.entries_mut().push(KdlEntry::new(value));
    node
}

/// Parse a child node shaped like `name "value"` and return the borrowed
/// string. Rejects child blocks, named entries, arity ≠ 1, and non-string
/// values.
pub fn parse_string_child(node: &KdlNode) -> Result<&str, FieldError> {
    if node.children().is_some() {
        return Err(FieldError::HasChildren { span: node.span() });
    }

    let entries = node.entries();
    for entry in entries {
        if entry.name().is_some() {
            return Err(FieldError::NamedEntry { span: entry.span() });
        }
    }

    let [entry] = entries else {
        return Err(FieldError::EntryCount { span: node.span() });
    };

    match entry.value() {
        KdlValue::String(s) => Ok(s.as_str()),
        _ => Err(FieldError::InvalidType {
            expected: "string",
            span: entry.span(),
        }),
    }
}

/// Parse a child node shaped like `name 8000`, verify the value lies inside
/// `range`, and convert it to `T` via `TryFrom<i128>`. Rejects child blocks,
/// named entries, arity ≠ 1, and non-integer values. Out-of-range integers
/// surface as `FieldError::OutOfRange`. The caller picks the range, so
/// custom bounds (e.g. `1024..=65535` for non-privileged ports) work the
/// same way as the natural type bound (`0..=u16::MAX.into()` for `u16`).
pub fn parse_integer_child<T>(node: &KdlNode, range: RangeInclusive<i128>) -> Result<T, FieldError>
where
    T: TryFrom<i128>,
{
    if node.children().is_some() {
        return Err(FieldError::HasChildren { span: node.span() });
    }

    let entries = node.entries();
    for entry in entries {
        if entry.name().is_some() {
            return Err(FieldError::NamedEntry { span: entry.span() });
        }
    }

    let [entry] = entries else {
        return Err(FieldError::EntryCount { span: node.span() });
    };

    let KdlValue::Integer(i) = entry.value() else {
        return Err(FieldError::InvalidType {
            expected: "integer",
            span: entry.span(),
        });
    };

    let out_of_range = || FieldError::OutOfRange {
        min: *range.start(),
        max: *range.end(),
        span: entry.span(),
    };

    if range.contains(i) {
        T::try_from(*i).map_err(|_| out_of_range())
    } else {
        Err(out_of_range())
    }
}

/// Convert a single child node (e.g. `engine "postgresql"`) into a typed
/// field value. The field name is read from `node.name().value()` for
/// error context. Implementors decide which `NodeError` shape is the
/// most precise — `FieldError`-wrapping shapes for scalar mismatches,
/// `UnknownVariant` for enums, etc.
///
/// This is the single conversion trait used by `set_field` and
/// `push_field`; local types should implement it directly instead of
/// `TryFrom<&KdlNode>`. The String impl below is the reason a local
/// trait exists at all (orphan rules block `impl TryFrom<&KdlNode> for
/// String`).
pub trait FromKdlNode: Sized {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError>;
}

impl FromKdlNode for String {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        parse_string_child(node)
            .map(str::to_owned)
            .map_err(|source| NodeError::invalid_field(node, source))
    }
}

impl FromKdlNode for u16 {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        parse_integer_child::<u16>(node, 0 ..= u16::MAX.into())
            .map_err(|source| NodeError::invalid_field(node, source))
    }
}

/// Reject any child block on `node`. The first child node is reported as
/// `UnknownField`. Used by parsers that allow no children at all (e.g. bare
/// enum variants like `proxy cloudflare`).
pub fn reject_children(node: &KdlNode) -> Result<(), NodeError> {
    if let Some(child) = node.children().and_then(|c| c.nodes().first()) {
        return Err(NodeError::UnknownField {
            name: child.name().value().to_owned(),
            span: child.span(),
        });
    }
    Ok(())
}

pub struct NodeField<T> {
    name: &'static str,
    value: Option<T>,
}

impl<T> NodeField<T>
where
    T: FromKdlNode,
{
    pub fn new(name: &'static str) -> Self {
        NodeField { name, value: None }
    }

    /// Parses `node` with `T::from_kdl_node` and stores the result.
    /// Returns `DuplicateField` if the value is already set.
    pub fn set(&mut self, node: &KdlNode) -> Result<(), NodeError> {
        if self.value.is_some() {
            return Err(NodeError::DuplicateField {
                name: node.name().value().to_owned(),
                span: node.span(),
            });
        }
        let value = T::from_kdl_node(node)?;

        self.value = Some(value);
        Ok(())
    }

    /// Parses `node` with `T::from_kdl_node`, validates it, and stores the result.
    /// Returns `DuplicateField` if the value is already set.
    pub fn set_with<F>(&mut self, node: &KdlNode, validate: F) -> Result<(), NodeError>
    where
        F: FnOnce(&T) -> Result<(), FieldError>,
    {
        if self.value.is_some() {
            return Err(NodeError::DuplicateField {
                name: node.name().value().to_owned(),
                span: node.span(),
            });
        }
        let value = T::from_kdl_node(node)?;

        validate(&value).map_err(|source| NodeError::InvalidField {
            name: node.name().value().to_owned(),
            span: node.span(),
            source,
        })?;

        self.value = Some(value);
        Ok(())
    }

    pub fn take_required(self, parent: &KdlNode) -> Result<T, NodeError> {
        self.value.ok_or_else(|| NodeError::MissingField {
            name: self.name.to_owned(),
            span: parent.span(),
        })
    }

    pub fn take_optional(self) -> Option<T> {
        self.value
    }
}

pub struct NodeList<T> {
    name: &'static str,
    list: Vec<T>,
}

impl<T> NodeList<T>
where
    T: FromKdlNode,
{
    pub fn new(name: &'static str) -> Self {
        NodeList {
            name,
            list: Vec::new(),
        }
    }

    /// Parses `node` with `T::from_kdl_node` and appends the result.
    pub fn push(&mut self, node: &KdlNode) -> Result<(), NodeError> {
        let value = T::from_kdl_node(node)?;
        self.list.push(value);
        Ok(())
    }

    /// Parses `node` with `T::from_kdl_node`, validates it, and appends the result.
    #[allow(dead_code)]
    pub fn push_with<F>(&mut self, node: &KdlNode, validate: F) -> Result<(), NodeError>
    where
        F: FnOnce(&T) -> Result<(), FieldError>,
    {
        let value = T::from_kdl_node(node)?;

        validate(&value).map_err(|source| NodeError::InvalidField {
            name: node.name().value().to_owned(),
            span: node.span(),
            source,
        })?;

        self.list.push(value);
        Ok(())
    }

    pub fn take_required(self, parent: &KdlNode) -> Result<Vec<T>, NodeError> {
        if self.list.is_empty() {
            Err(NodeError::MissingField {
                name: self.name.to_owned(),
                span: parent.span(),
            })
        } else {
            Ok(self.list)
        }
    }

    pub fn take_optional(self) -> Vec<T> {
        self.list
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
    fn u16_happy_path() {
        let n = node("port 8000");
        let port = u16::from_kdl_node(&n).unwrap();
        assert_eq!(port, 8000);
    }

    #[test]
    fn u16_zero_and_max() {
        let zero = u16::from_kdl_node(&node("port 0")).unwrap();
        let max = u16::from_kdl_node(&node("port 65535")).unwrap();
        assert_eq!((zero, max), (0, 65535));
    }

    #[test]
    fn u16_out_of_range() {
        let err = u16::from_kdl_node(&node("port 70000")).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::OutOfRange { min: 0, max: 65535, .. },
                    ..
                } if name == "port",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn u16_negative() {
        let err = u16::from_kdl_node(&node("port -1")).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    source: FieldError::OutOfRange {
                        min: 0,
                        max: 65535,
                        ..
                    },
                    ..
                },
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn u16_wrong_type() {
        let err = u16::from_kdl_node(&node(r#"port "8000""#)).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    source: FieldError::InvalidType {
                        expected: "integer",
                        ..
                    },
                    ..
                },
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn u16_named_entry_rejected() {
        let err = u16::from_kdl_node(&node("port value=8000")).unwrap_err();
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

    #[test]
    fn u16_rejects_children() {
        let err = u16::from_kdl_node(&node("port 8000 { extra }")).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    source: FieldError::HasChildren { .. },
                    ..
                },
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn u16_missing_value() {
        let err = u16::from_kdl_node(&node("port")).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    source: FieldError::EntryCount { .. },
                    ..
                },
            ),
            "unexpected: {err:?}",
        );
    }
}
