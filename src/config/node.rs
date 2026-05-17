use kdl::{
    KdlNode,
    KdlValue,
};

use super::{
    ConfigNodeError,
    FieldError,
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
/// field value. Implementors decide which `ConfigNodeError` shape is the
/// most precise — `FieldError`-wrapping shapes for scalar mismatches,
/// `UnknownVariant` for enums, etc.
pub(crate) trait FromConfigNode: Sized {
    fn from_config_node(node: &KdlNode, name: &str) -> Result<Self, ConfigNodeError>;
}

impl FromConfigNode for String {
    fn from_config_node(node: &KdlNode, name: &str) -> Result<Self, ConfigNodeError> {
        parse_string_child(node)
            .map(str::to_owned)
            .map_err(|source| ConfigNodeError::InvalidField {
                name: name.to_owned(),
                span: node.span(),
                source,
            })
    }
}

/// Assign `child` into `target`, dispatching to `T::from_config_node`.
/// Rejects re-assignment with `DuplicateField`.
pub fn set_field<T: FromConfigNode>(
    target: &mut Option<T>,
    child: &KdlNode,
    name: &str,
) -> Result<(), ConfigNodeError> {
    if target.is_some() {
        return Err(ConfigNodeError::DuplicateField {
            name: name.to_owned(),
            span: child.span(),
        });
    }

    *target = Some(T::from_config_node(child, name)?);
    Ok(())
}
