use kdl::{
    KdlEntry,
    KdlNode,
    KdlValue,
};

use super::{
    FieldError,
    NodeError,
};

/// Convert a single positional entry of a wrapper node (e.g. the `"run-tasks"`
/// in `timer "run-tasks" { ... }`) into a typed value.
pub trait FromKdlArg: Sized {
    fn from_kdl_arg(entry: &KdlEntry) -> Result<Self, FieldError>;
}

impl FromKdlArg for String {
    fn from_kdl_arg(entry: &KdlEntry) -> Result<Self, FieldError> {
        if entry.name().is_some() {
            return Err(FieldError::NamedEntry { span: entry.span() });
        }

        match entry.value() {
            KdlValue::String(s) => Ok(s.clone()),
            _ => Err(FieldError::InvalidType {
                expected: "string",
                span: entry.span(),
            }),
        }
    }
}

pub fn kdl_args_check_arity<'a>(
    node: &'a KdlNode,
    names: &[&'static str],
) -> Result<&'a [KdlEntry], NodeError> {
    let entries = node.entries();
    let expected_len = names.len();
    if entries.len() < expected_len {
        Err(NodeError::MissingArg {
            name: names[entries.len()],
            span: node.span(),
        })
    } else if entries.len() > expected_len {
        Err(NodeError::UnexpectedArg {
            span: entries[expected_len].span(),
        })
    } else {
        Ok(entries)
    }
}

/// Extract positional arguments of a wrapper node into typed locals.
///
/// Three forms:
///
/// ```ignore
/// kdl_args!(node)?;                                // reject any positional args
/// let name = kdl_args!(node, name: String)?;       // exactly one, returns T
/// let (a, b) = kdl_args!(node, a: String, b: String)?;  // N ≥ 2, returns tuple
/// ```
///
/// Arity mismatches surface as `NodeError::MissingArg` (too few - names
/// the first missing field) or `NodeError::UnexpectedArg` (too many -
/// span points at the first extra entry). Type and named-entry errors flow
/// through `FromKdlArg` implementations as `NodeError::InvalidField`.
#[macro_export]
macro_rules! kdl_args {
    // 0 args: reject any positional entry on the node.
    ($node:expr) => {{
        $crate::config::kdl_args_check_arity($node, &[]).and_then(|_| {
            Ok(())
        })
    }};

    // 1 arg: return T
    ($node:expr, $name:ident : $ty:ty $(,)?) => {{
        const NAMES: &[&'static str] = &[stringify!($name)];
        $crate::config::kdl_args_check_arity($node, NAMES).and_then(|entries| {
            <$ty as $crate::config::FromKdlArg>::from_kdl_arg(&entries[0]).map_err(|source| $crate::config::NodeError::InvalidField {
                name: $node.name().value().to_owned(),
                span: $node.span(),
                source,
            })
        })
    }};

    // 2+ args: return tuple
    ($node:expr, $($name:ident : $ty:ty),+ $(,)?) => {{
        const NAMES: &[&'static str] = &[$(stringify!($name),)+];
        $crate::config::kdl_args_check_arity($node, NAMES).and_then(|entries| {
            let mut __i = 0;
            let tuple = (
                $({
                    let v = <$ty as $crate::config::FromKdlArg>::from_kdl_arg(
                        &entries[__i],
                    ).map_err(|source| $crate::config::NodeError::InvalidField {
                        name: $node.name().value().to_owned(),
                        span: $node.span(),
                        source,
                    })?;
                    __i += 1;
                    v
                },)+
            );
            Ok(tuple)
        })
    }};
}

#[cfg(test)]
mod tests {
    use kdl::KdlDocument;

    use super::*;

    fn node(src: &str) -> kdl::KdlNode {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        doc.nodes()
            .first()
            .expect("test KDL must have one node")
            .clone()
    }

    #[test]
    fn no_args_accepts_bare_node() {
        let n = node("wrapper");
        kdl_args!(&n).unwrap();
    }

    #[test]
    fn no_args_accepts_node_with_children_only() {
        let n = node("wrapper { child }");
        kdl_args!(&n).unwrap();
    }

    #[test]
    fn no_args_rejects_positional() {
        let n = node(r#"wrapper "stray""#);
        let err = kdl_args!(&n).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn one_arg_string_happy_path() {
        let n = node(r#"timer "run-tasks""#);
        let name: String = kdl_args!(&n, name: String).unwrap();
        assert_eq!(name, "run-tasks");
    }

    #[test]
    fn one_arg_missing_names_field() {
        let n = node("timer");
        let err = kdl_args!(&n, name: String).unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "name", .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn one_arg_extra_rejected() {
        let n = node(r#"timer "run-tasks" "extra""#);
        let err = kdl_args!(&n, name: String).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn one_arg_wrong_type() {
        let n = node("timer 5");
        let err = kdl_args!(&n, name: String).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "name",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn one_arg_named_entry_rejected() {
        let n = node(r#"timer name="run-tasks""#);
        let err = kdl_args!(&n, name: String).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::NamedEntry { .. },
                    ..
                } if name == "name",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn two_args_happy_path() {
        let n = node(r#"route "reverse-proxy" "/api""#);
        let (kind, path): (String, String) = kdl_args!(&n, kind: String, path: String).unwrap();
        assert_eq!(kind, "reverse-proxy");
        assert_eq!(path, "/api");
    }

    #[test]
    fn two_args_missing_second_names_it() {
        let n = node(r#"route "reverse-proxy""#);
        let err = kdl_args!(&n, kind: String, path: String).unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "path", .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn two_args_missing_first_names_it() {
        let n = node("route");
        let err = kdl_args!(&n, kind: String, path: String).unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "kind", .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn two_args_extra_rejected() {
        let n = node(r#"route "a" "b" "c""#);
        let err = kdl_args!(&n, kind: String, path: String).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn three_args_happy_path() {
        let n = node(r#"x "a" "b" "c""#);
        let (a, b, c): (String, String, String) =
            kdl_args!(&n, a: String, b: String, c: String).unwrap();
        assert_eq!((a.as_str(), b.as_str(), c.as_str()), ("a", "b", "c"));
    }

    #[test]
    fn three_args_missing_third_names_it() {
        let n = node(r#"x "a" "b""#);
        let err = kdl_args!(&n, a: String, b: String, c: String).unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "c", .. }),
            "unexpected: {err:?}",
        );
    }
}
