use kdl::{
    KdlEntry,
    KdlValue,
};

use super::{
    ConfigNodeError,
    FieldError,
};

/// Convert a single positional entry of a wrapper node (e.g. the `"run-tasks"`
/// in `timer "run-tasks" { ... }`) into a typed value. Implementors are
/// responsible for rejecting named entries and reporting type mismatches via
/// `ConfigNodeError::InvalidField` with the caller-supplied field name.
#[allow(dead_code)] // used via kdl_args! macro; call-site migration is the next PR
pub(crate) trait FromConfigArg: Sized {
    fn from_config_arg(entry: &KdlEntry, field: &str) -> Result<Self, ConfigNodeError>;
}

impl FromConfigArg for String {
    fn from_config_arg(entry: &KdlEntry, field: &str) -> Result<Self, ConfigNodeError> {
        if entry.name().is_some() {
            return Err(ConfigNodeError::InvalidField {
                name: field.to_owned(),
                span: entry.span(),
                source: FieldError::NamedEntry { span: entry.span() },
            });
        }

        match entry.value() {
            KdlValue::String(s) => Ok(s.clone()),
            _ => Err(ConfigNodeError::InvalidField {
                name: field.to_owned(),
                span: entry.span(),
                source: FieldError::InvalidType {
                    expected: "string",
                    span: entry.span(),
                },
            }),
        }
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
/// Arity mismatches surface as `ConfigNodeError::MissingArg` (too few — names
/// the first missing field) or `ConfigNodeError::UnexpectedArg` (too many —
/// span points at the first extra entry). Type and named-entry errors flow
/// through `FromConfigArg` implementations as `ConfigNodeError::InvalidField`.
#[macro_export]
macro_rules! kdl_args {
    // 0 args: reject any positional entry on the node.
    ($node:expr) => {{
        let __node: &::kdl::KdlNode = $node;
        match __node.entries().first() {
            Some(entry) => Err::<(), $crate::config::ConfigNodeError>(
                $crate::config::ConfigNodeError::UnexpectedArg { span: entry.span() },
            ),
            None => Ok::<(), $crate::config::ConfigNodeError>(()),
        }
    }};

    // 1 arg: return T
    ($node:expr, $name:ident : $ty:ty $(,)?) => {{
        let __node: &::kdl::KdlNode = $node;
        let __entries = __node.entries();
        const __NAME: &'static str = stringify!($name);
        let __r: ::core::result::Result<_, $crate::config::ConfigNodeError> = (|| {
            if __entries.is_empty() {
                return Err($crate::config::ConfigNodeError::MissingArg {
                    name: __NAME,
                    span: __node.span(),
                });
            }
            if __entries.len() > 1 {
                return Err($crate::config::ConfigNodeError::UnexpectedArg {
                    span: __entries[1].span(),
                });
            }
           let __v= <$ty as $crate::config::FromConfigArg>::from_config_arg(&__entries[1], __NAME)?;
            Ok(__v)
        })();
        __r
    }};

    // 2+ args: return tuple
    ($node:expr, $($more_name:ident : $more_ty:ty),+ $(,)?) => {{
        let __node: &::kdl::KdlNode = $node;
        let __entries = __node.entries();
        const __NAMES: &[&'static str] = &[
            $(stringify!($more_name),)*
        ];
        const __EXPECTED: usize = __NAMES.len();
        let __r: ::core::result::Result<_, $crate::config::ConfigNodeError> = (|| {
            if __entries.len() < __EXPECTED {
                return Err($crate::config::ConfigNodeError::MissingArg {
                    name: __NAMES[__entries.len()],
                    span: __node.span(),
                });
            }
            if __entries.len() > __EXPECTED {
                return Err($crate::config::ConfigNodeError::UnexpectedArg {
                    span: __entries[__EXPECTED].span(),
                });
            }
            let mut __i = 0_usize;
            Ok((
                $({
                    let __v = <$more_ty as $crate::config::FromConfigArg>::from_config_arg(
                        &__entries[__i],
                        __NAMES[__i],
                    )?;
                    __i += 1;
                    __v
                },)*
            ))
        })();
        __r
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
            matches!(err, ConfigNodeError::UnexpectedArg { .. }),
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
            matches!(err, ConfigNodeError::MissingArg { name: "name", .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn one_arg_extra_rejected() {
        let n = node(r#"timer "run-tasks" "extra""#);
        let err = kdl_args!(&n, name: String).unwrap_err();
        assert!(
            matches!(err, ConfigNodeError::UnexpectedArg { .. }),
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
                ConfigNodeError::InvalidField {
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
                ConfigNodeError::InvalidField {
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
            matches!(err, ConfigNodeError::MissingArg { name: "path", .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn two_args_missing_first_names_it() {
        let n = node("route");
        let err = kdl_args!(&n, kind: String, path: String).unwrap_err();
        assert!(
            matches!(err, ConfigNodeError::MissingArg { name: "kind", .. }),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn two_args_extra_rejected() {
        let n = node(r#"route "a" "b" "c""#);
        let err = kdl_args!(&n, kind: String, path: String).unwrap_err();
        assert!(
            matches!(err, ConfigNodeError::UnexpectedArg { .. }),
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
            matches!(err, ConfigNodeError::MissingArg { name: "c", .. }),
            "unexpected: {err:?}",
        );
    }
}
