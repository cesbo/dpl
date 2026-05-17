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
