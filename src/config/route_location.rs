use std::fmt;

use kdl::{
    KdlEntry,
    KdlValue,
};
use miette::Diagnostic;
use serde::{
    Serialize,
    Serializer,
};
use thiserror::Error;

use super::{
    FieldError,
    FromKdlArg,
};

#[derive(Diagnostic, Error, Debug, Clone, PartialEq, Eq)]
#[error("invalid route location '{input}'")]
#[diagnostic(help(
    "'/' or '/segment[/segment...]' with ASCII alphanumeric, '-', '_', '.'; no trailing '/'; no \
     '.'/'..' segments"
))]
pub struct InvalidRouteLocation {
    pub input: String,
}

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RouteLocation(String);

impl RouteLocation {
    pub fn new(raw: &str) -> Result<Self, InvalidRouteLocation> {
        if !Self::is_valid(raw) {
            return Err(InvalidRouteLocation {
                input: raw.to_owned(),
            });
        }
        Ok(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(raw: &str) -> bool {
        if raw == "/" {
            return true;
        }

        if !raw.starts_with('/') || raw.ends_with('/') {
            return false;
        }

        for segment in raw[1 ..].split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return false;
            }

            let valid = segment
                .as_bytes()
                .iter()
                .all(|&b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
            if !valid {
                return false;
            }
        }

        true
    }
}

impl fmt::Display for RouteLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for RouteLocation {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

impl From<&RouteLocation> for KdlEntry {
    fn from(value: &RouteLocation) -> Self {
        KdlEntry::new(String::from(value.as_str()))
    }
}

impl FromKdlArg for RouteLocation {
    fn from_kdl_arg(entry: &KdlEntry) -> Result<Self, FieldError> {
        if entry.name().is_some() {
            return Err(FieldError::NamedEntry { span: entry.span() });
        }

        let KdlValue::String(s) = entry.value() else {
            return Err(FieldError::InvalidType {
                expected: "string",
                span: entry.span(),
            });
        };

        Self::new(s).map_err(|source| FieldError::InvalidRouteLocation {
            span: entry.span(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use kdl::{
        KdlDocument,
        KdlNode,
    };

    use super::*;
    use crate::{
        config::NodeError,
        kdl_args,
    };

    fn node(src: &str) -> KdlNode {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        doc.nodes()
            .first()
            .expect("test KDL must have one node")
            .clone()
    }

    #[test]
    fn new_accepts_valid() {
        for ok in ["/", "/api", "/api/v1", "/a-b_c.d", "/billing/static"] {
            assert_eq!(RouteLocation::new(ok).unwrap().as_str(), ok);
        }
    }

    #[test]
    fn new_rejects_invalid() {
        for bad in [
            "", "api", "/api/", "//", "/api//v1", "/.", "/..", "/api/..", "/ api", "/api?x=1",
            "/Ümlaut",
        ] {
            let err = RouteLocation::new(bad).unwrap_err();
            assert_eq!(err.input, bad, "expected error to carry input {bad:?}");
        }
    }

    #[test]
    fn from_arg_happy() {
        let n = node(r#"route reverse_proxy "/api""#);
        let (_variant, location): (String, RouteLocation) =
            kdl_args!(&n, variant: String, location: RouteLocation).unwrap();
        assert_eq!(location.as_str(), "/api");
    }

    #[test]
    fn from_arg_happy_root() {
        let n = node(r#"route reverse_proxy "/""#);
        let (_variant, location): (String, RouteLocation) =
            kdl_args!(&n, variant: String, location: RouteLocation).unwrap();
        assert_eq!(location.as_str(), "/");
    }

    #[test]
    fn from_arg_rejects_invalid_value() {
        let n = node(r#"route reverse_proxy "no-leading-slash""#);
        let err = kdl_args!(&n, variant: String, location: RouteLocation).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidRouteLocation {
                        source: InvalidRouteLocation { input },
                        ..
                    },
                    ..
                } if name == "route" && input == "no-leading-slash",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn from_arg_rejects_wrong_type() {
        let n = node("route reverse_proxy 5");
        let err = kdl_args!(&n, variant: String, location: RouteLocation).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType {
                        expected: "string",
                        ..
                    },
                    ..
                } if name == "route",
            ),
            "unexpected: {err:?}",
        );
    }

    #[test]
    fn from_arg_rejects_named_entry() {
        let n = node(r#"route reverse_proxy location="/api""#);
        let err = kdl_args!(&n, variant: String, location: RouteLocation).unwrap_err();
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
