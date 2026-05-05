use std::collections::BTreeMap;

use serde::{
    Deserialize,
    Deserializer,
};
use thiserror::Error;

use crate::validate;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValueError {
    #[error("unterminated reference at position {pos}: missing '}}'")]
    UnterminatedRef { pos: usize },

    #[error("bare '$' at position {pos}: use '$$' for a literal '$' or '${{ns:name}}' for a reference")]
    BareDollar { pos: usize },

    #[error("malformed reference at position {pos}: expected '${{ns:name}}'")]
    MalformedRef { pos: usize },

    #[error("unknown namespace '{ns}' at position {pos}")]
    UnknownNamespace { pos: usize, ns: String },

    #[error("invalid name '{name}' for namespace '{ns}' at position {pos}")]
    InvalidName {
        pos: usize,
        ns: &'static str,
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ns {
    Secret,
}

impl Ns {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "secret" => Some(Self::Secret),
            _ => None,
        }
    }

    fn as_str(&self) -> &'static str {
        match self {
            Self::Secret => "secret",
        }
    }

    fn validate_name(&self, name: &str) -> bool {
        match self {
            Self::Secret => validate::secret_name(name),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Ref { ns: Ns, name: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Value(Vec<Segment>);

impl Value {
    pub fn parse(input: &str) -> Result<Self, ValueError> {
        let bytes = input.as_bytes();
        let mut segments: Vec<Segment> = Vec::new();
        let mut literal = String::new();
        let mut i = 0;

        while i < bytes.len() {
            let b = bytes[i];
            if b != b'$' {
                literal.push(bytes[i] as char);
                i += 1;
                continue;
            }

            let next = bytes.get(i + 1).copied();
            match next {
                Some(b'$') => {
                    literal.push('$');
                    i += 2;
                }
                Some(b'{') => {
                    if !literal.is_empty() {
                        segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    }
                    let start = i;
                    let body_start = i + 2;
                    let end = bytes[body_start ..]
                        .iter()
                        .position(|&c| c == b'}')
                        .ok_or(ValueError::UnterminatedRef { pos: start })?;
                    let body = &input[body_start .. body_start + end];
                    segments.push(parse_ref(body, start)?);
                    i = body_start + end + 1;
                }
                _ => return Err(ValueError::BareDollar { pos: i }),
            }
        }

        if !literal.is_empty() {
            segments.push(Segment::Literal(literal));
        }

        Ok(Self(segments))
    }

    pub fn references(&self) -> impl Iterator<Item = (&Ns, &str)> {
        self.0.iter().filter_map(|seg| match seg {
            Segment::Ref { ns, name } => Some((ns, name.as_str())),
            Segment::Literal(_) => None,
        })
    }

    pub fn render(&self, values: &BTreeMap<Ns, BTreeMap<String, String>>) -> String {
        let mut out = String::new();
        for seg in &self.0 {
            match seg {
                Segment::Literal(s) => out.push_str(s),
                Segment::Ref { ns, name } => {
                    if let Some(v) = values.get(ns).and_then(|m| m.get(name)) {
                        out.push_str(v);
                    }
                }
            }
        }
        out
    }
}

fn parse_ref(body: &str, pos: usize) -> Result<Segment, ValueError> {
    let (ns_raw, name) = body
        .split_once(':')
        .ok_or(ValueError::MalformedRef { pos })?;

    if ns_raw.is_empty() || name.is_empty() {
        return Err(ValueError::MalformedRef { pos });
    }

    let ns = Ns::parse(ns_raw).ok_or_else(|| ValueError::UnknownNamespace {
        pos,
        ns: ns_raw.to_owned(),
    })?;

    if !ns.validate_name(name) {
        return Err(ValueError::InvalidName {
            pos,
            ns: ns.as_str(),
            name: name.to_owned(),
        });
    }

    Ok(Segment::Ref { ns, name: name.to_owned() })
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(s: &str) -> Segment {
        Segment::Literal(s.to_owned())
    }

    fn sref(name: &str) -> Segment {
        Segment::Ref { ns: Ns::Secret, name: name.to_owned() }
    }

    #[test]
    fn parse_literal_only() {
        let v = Value::parse("hello world").unwrap();
        assert_eq!(v.0, vec![lit("hello world")]);
    }

    #[test]
    fn parse_empty() {
        let v = Value::parse("").unwrap();
        assert_eq!(v.0, Vec::<Segment>::new());
    }

    #[test]
    fn parse_single_ref() {
        let v = Value::parse("${secret:my-key}").unwrap();
        assert_eq!(v.0, vec![sref("my-key")]);
    }

    #[test]
    fn parse_mixed() {
        let v = Value::parse("postgres://app:${secret:db}@host/${secret:db-name}?x=1").unwrap();
        assert_eq!(
            v.0,
            vec![
                lit("postgres://app:"),
                sref("db"),
                lit("@host/"),
                sref("db-name"),
                lit("?x=1"),
            ]
        );
    }

    #[test]
    fn parse_escape() {
        let v = Value::parse("cost: $$5 / $$").unwrap();
        assert_eq!(v.0, vec![lit("cost: $5 / $")]);
    }

    #[test]
    fn parse_escape_then_ref() {
        let v = Value::parse("$$${secret:k}").unwrap();
        assert_eq!(v.0, vec![lit("$"), sref("k")]);
    }

    #[test]
    fn parse_unterminated_ref() {
        assert!(matches!(
            Value::parse("foo ${secret:bar"),
            Err(ValueError::UnterminatedRef { pos: 4 })
        ));
    }

    #[test]
    fn parse_invalid_secret_name_underscore() {
        let err = Value::parse("${secret:my_key}").unwrap_err();
        assert!(matches!(err, ValueError::InvalidName { ns: "secret", .. }));
    }

    #[test]
    fn parse_bare_dollar() {
        assert!(matches!(
            Value::parse("price $5"),
            Err(ValueError::BareDollar { pos: 6 })
        ));
    }

    #[test]
    fn parse_trailing_dollar() {
        assert!(matches!(
            Value::parse("end$"),
            Err(ValueError::BareDollar { pos: 3 })
        ));
    }

    #[test]
    fn parse_unknown_namespace() {
        let err = Value::parse("${env:HOME}").unwrap_err();
        assert!(matches!(err, ValueError::UnknownNamespace { .. }));
    }

    #[test]
    fn parse_empty_name() {
        assert!(matches!(
            Value::parse("${secret:}"),
            Err(ValueError::MalformedRef { .. })
        ));
    }

    #[test]
    fn parse_empty_ns() {
        assert!(matches!(
            Value::parse("${:foo}"),
            Err(ValueError::MalformedRef { .. })
        ));
    }

    #[test]
    fn parse_no_colon() {
        assert!(matches!(
            Value::parse("${secret}"),
            Err(ValueError::MalformedRef { .. })
        ));
    }

    #[test]
    fn parse_invalid_secret_name() {
        let err = Value::parse("${secret:Bad Name}").unwrap_err();
        assert!(matches!(err, ValueError::InvalidName { ns: "secret", .. }));
    }

    #[test]
    fn render_literal() {
        let v = Value::parse("hello").unwrap();
        assert_eq!(v.render(&BTreeMap::new()), "hello");
    }

    #[test]
    fn render_with_secrets() {
        let v = Value::parse("u:${secret:user};p:${secret:pass}").unwrap();
        let mut secrets = BTreeMap::new();
        secrets.insert("user".to_owned(), "alice".to_owned());
        secrets.insert("pass".to_owned(), "s3cr3t".to_owned());
        let mut values = BTreeMap::new();
        values.insert(Ns::Secret, secrets);
        assert_eq!(v.render(&values), "u:alice;p:s3cr3t");
    }

    #[test]
    fn references_iter() {
        let v = Value::parse("a${secret:x}b${secret:y}c${secret:x}").unwrap();
        let refs: Vec<(&Ns, &str)> = v.references().collect();
        assert_eq!(
            refs,
            vec![(&Ns::Secret, "x"), (&Ns::Secret, "y"), (&Ns::Secret, "x")]
        );
    }

    #[test]
    fn parse_namespaced_secret() {
        let v = Value::parse("${secret:db/prod-password}").unwrap();
        assert_eq!(v.0, vec![sref("db/prod-password")]);
    }
}
