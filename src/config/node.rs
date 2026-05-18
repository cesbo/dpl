use std::ops::RangeInclusive;

use kdl::{
    KdlNode,
    KdlValue,
};

use super::{
    FieldError,
    NodeError,
};

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
pub(crate) trait FromKdlNode: Sized {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError>;
}

impl FromKdlNode for String {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        parse_string_child(node)
            .map(str::to_owned)
            .map_err(|source| NodeError::InvalidField {
                name: node.name().value().to_owned(),
                span: node.span(),
                source,
            })
    }
}

impl FromKdlNode for u16 {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        parse_integer_child::<u16>(node, 0 ..= u16::MAX.into()).map_err(|source| {
            NodeError::InvalidField {
                name: node.name().value().to_owned(),
                span: node.span(),
                source,
            }
        })
    }
}

/// Assign `child` into `target`, dispatching to `T::from_kdl_node`.
/// Rejects re-assignment with `DuplicateField`.
pub fn set_field<T: FromKdlNode>(target: &mut Option<T>, child: &KdlNode) -> Result<(), NodeError> {
    if target.is_some() {
        return Err(NodeError::DuplicateField {
            name: child.name().value().to_owned(),
            span: child.span(),
        });
    }

    *target = Some(T::from_kdl_node(child)?);
    Ok(())
}

/// Append `child` to `target`, dispatching to `T::from_kdl_node`. Used for
/// fields where the same child name may appear repeatedly (e.g. `file
/// "a"; file "b"`).
pub fn push_field<T: FromKdlNode>(target: &mut Vec<T>, child: &KdlNode) -> Result<(), NodeError> {
    target.push(T::from_kdl_node(child)?);
    Ok(())
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
