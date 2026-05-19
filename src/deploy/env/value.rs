use kdl::{
    KdlNode,
    KdlValue,
};

use crate::{
    MainContext,
    config::{
        FieldError,
        FromKdlNode,
        NodeError,
        ResourceName,
        TemplateError,
    },
    deploy::unit,
    error::{
        Location,
        RefError,
    },
    validate,
};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Ns {
    Secret,
    Unit(String),
}

impl Ns {
    fn parse(raw: &str) -> Option<Self> {
        if raw == "secret" {
            return Some(Self::Secret);
        }

        if ResourceName::is_valid(raw) {
            return Some(Self::Unit(raw.to_owned()));
        }

        None
    }

    fn as_str(&self) -> &str {
        match self {
            Self::Secret => "secret",
            Self::Unit(name) => name,
        }
    }

    fn validate_name(&self, name: &str) -> bool {
        match self {
            Self::Secret => validate::secret_name(name),
            Self::Unit(_) => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Segment {
    Literal(String),
    Dollar,
    Ref { ns: Ns, name: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Value(Vec<Segment>);

impl Value {
    pub fn parse(input: &str) -> Result<Self, TemplateError> {
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
                    if !literal.is_empty() {
                        segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    }
                    segments.push(Segment::Dollar);
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
                        .ok_or(TemplateError::UnterminatedRef { pos: start })?;
                    let body = &input[body_start .. body_start + end];
                    segments.push(parse_ref(body, start)?);
                    i = body_start + end + 1;
                }
                _ => return Err(TemplateError::BareDollar { pos: i }),
            }
        }

        if !literal.is_empty() {
            segments.push(Segment::Literal(literal));
        }

        Ok(Self(segments))
    }

    pub(crate) fn literal(s: String) -> Self {
        Self(vec![Segment::Literal(s)])
    }

    /// Render back to the wire string form.
    pub fn as_template(&self) -> String {
        let mut out = String::new();
        for seg in &self.0 {
            match seg {
                Segment::Literal(s) => out.push_str(s),
                Segment::Dollar => out.push_str("$$"),
                Segment::Ref { ns, name } => {
                    out.push_str("${");
                    out.push_str(ns.as_str());
                    out.push(':');
                    out.push_str(name);
                    out.push('}');
                }
            }
        }
        out
    }

    pub fn references(&self) -> impl Iterator<Item = (&Ns, &str)> {
        self.0.iter().filter_map(|seg| match seg {
            Segment::Ref { ns, name } => Some((ns, name.as_str())),
            Segment::Literal(_) | Segment::Dollar => None,
        })
    }

    pub fn render(&self, ctx: &MainContext) -> Result<String, RefError> {
        let mut out = String::new();
        for seg in &self.0 {
            match seg {
                Segment::Literal(s) => out.push_str(s),
                Segment::Dollar => out.push('$'),
                Segment::Ref { ns, name } => {
                    let token = format!("${{{}:{}}}", ns.as_str(), name);
                    let value = match ns {
                        Ns::Secret => ctx
                            .resolve_secret(name)
                            .map_err(RefError::from)
                            .map_err(|e| e.at(Location::token(&token)))?,
                        Ns::Unit(unit_name) => unit::resolve_export(ctx, unit_name, name)
                            .map_err(|e| e.at(Location::token(&token)))?,
                    };
                    out.push_str(&value);
                }
            }
        }
        Ok(out)
    }
}

fn parse_ref(body: &str, pos: usize) -> Result<Segment, TemplateError> {
    let (ns_raw, name) = body
        .split_once(':')
        .ok_or(TemplateError::MalformedRef { pos })?;

    if ns_raw.is_empty() || name.is_empty() {
        return Err(TemplateError::MalformedRef { pos });
    }

    let ns = Ns::parse(ns_raw).ok_or_else(|| TemplateError::UnknownNamespace {
        pos,
        ns: ns_raw.to_owned(),
    })?;

    if !ns.validate_name(name) {
        return Err(TemplateError::InvalidName {
            pos,
            ns: ns.as_str().to_owned(),
            name: name.to_owned(),
        });
    }

    Ok(Segment::Ref {
        ns,
        name: name.to_owned(),
    })
}

impl FromKdlNode for Value {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        if node.children().is_some() {
            return Err(NodeError::invalid_field(
                node,
                FieldError::HasChildren { span: node.span() },
            ));
        }

        let entries = node.entries();
        for entry in entries {
            if entry.name().is_some() {
                return Err(NodeError::invalid_field(
                    node,
                    FieldError::NamedEntry { span: entry.span() },
                ));
            }
        }

        let [entry] = entries else {
            return Err(NodeError::invalid_field(
                node,
                FieldError::EntryCount { span: node.span() },
            ));
        };

        match entry.value() {
            KdlValue::String(s) => Value::parse(s).map_err(|source| {
                NodeError::invalid_field(node, FieldError::InvalidTemplate {
                    span: entry.span(),
                    source,
                })
            }),
            KdlValue::Integer(i) => Ok(Value::literal(i.to_string())),
            KdlValue::Float(f) => Ok(Value::literal(f.to_string())),
            KdlValue::Bool(b) => Ok(Value::literal(if *b { "true" } else { "false" }.to_owned())),
            KdlValue::Null => Err(NodeError::invalid_field(node, FieldError::InvalidType {
                expected: "non-null value",
                span: entry.span(),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secret::SecretError;

    fn lit(s: &str) -> Segment {
        Segment::Literal(s.to_owned())
    }

    fn sref(name: &str) -> Segment {
        Segment::Ref {
            ns: Ns::Secret,
            name: name.to_owned(),
        }
    }

    fn uref(unit: &str, key: &str) -> Segment {
        Segment::Ref {
            ns: Ns::Unit(unit.to_owned()),
            name: key.to_owned(),
        }
    }

    #[test]
    fn parse_literal_only() {
        let v = Value::parse("hello world").unwrap();
        assert_eq!(v.0, vec![lit("hello world")]);
    }

    #[test]
    fn parse_single_ref() {
        let v = Value::parse("${secret:my-key}").unwrap();
        assert_eq!(v.0, vec![sref("my-key")]);
    }

    #[test]
    fn parse_escape() {
        let v = Value::parse("cost: $$5 / $$").unwrap();
        assert_eq!(
            v.0,
            vec![lit("cost: "), Segment::Dollar, lit("5 / "), Segment::Dollar]
        );
    }

    #[test]
    fn parse_escape_then_ref() {
        let v = Value::parse("$$${secret:k}").unwrap();
        assert_eq!(v.0, vec![Segment::Dollar, sref("k")]);
    }

    #[test]
    fn parse_unterminated_ref() {
        assert!(matches!(
            Value::parse("foo ${secret:bar"),
            Err(TemplateError::UnterminatedRef { pos: 4 })
        ));
    }

    #[test]
    fn parse_bare_dollar() {
        assert!(matches!(
            Value::parse("price $5"),
            Err(TemplateError::BareDollar { pos: 6 })
        ));

        assert!(matches!(
            Value::parse("end$"),
            Err(TemplateError::BareDollar { pos: 3 })
        ));
    }

    #[test]
    fn parse_malformed_ref() {
        for input in ["${secret:}", "${:foo}", "${secret}"] {
            assert!(
                matches!(Value::parse(input), Err(TemplateError::MalformedRef { .. })),
                "expected MalformedRef for {input:?}",
            );
        }
    }

    #[test]
    fn parse_invalid_secret_name() {
        let err = Value::parse("${secret:Bad Name}").unwrap_err();
        assert!(matches!(err, TemplateError::InvalidName { ref ns, .. } if ns == "secret"));
    }

    #[test]
    fn parse_namespaced_secret() {
        let v = Value::parse("${secret:db/prod-password}").unwrap();
        assert_eq!(v.0, vec![sref("db/prod-password")]);
    }

    #[test]
    fn parse_unit_ref() {
        let v = Value::parse("${pg-main:port}").unwrap();
        assert_eq!(v.0, vec![uref("pg-main", "port")]);
    }

    #[test]
    fn parse_mixed_refs() {
        let v =
            Value::parse("postgres://${app-db:user}:${secret:app-pass}@${pg-main:host}").unwrap();
        assert_eq!(
            v.0,
            vec![
                lit("postgres://"),
                uref("app-db", "user"),
                lit(":"),
                sref("app-pass"),
                lit("@"),
                uref("pg-main", "host"),
            ]
        );
    }

    #[test]
    fn parse_unknown_namespace() {
        let err = Value::parse("${PgMain:port}").unwrap_err();
        assert!(matches!(err, TemplateError::UnknownNamespace { ref ns, .. } if ns == "PgMain"));
    }

    #[test]
    fn as_template_roundtrip() {
        for input in [
            "",
            "hello world",
            "${secret:my-key}",
            "postgres://app:${secret:db}@host/${secret:db-name}?x=1",
            "cost: $$5 / $$",
            "$$${secret:k}",
            "${app-db:user}-${secret:pw}",
        ] {
            let v = Value::parse(input).unwrap();
            assert_eq!(v.as_template(), input, "roundtrip mismatch for {input:?}");
            assert_eq!(Value::parse(&v.as_template()).unwrap(), v);
        }
    }

    #[test]
    fn references_iter() {
        let v = Value::parse("${secret:x}-${pg-main:port}").unwrap();
        let refs: Vec<(&Ns, &str)> = v.references().collect();
        assert_eq!(
            refs,
            vec![
                (&Ns::Secret, "x"),
                (&Ns::Unit("pg-main".to_owned()), "port"),
            ]
        );
    }

    fn write_db_unit(base: &std::path::Path) {
        let dir = base.join("app-db");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("config.kdl"),
            r#"
db {
    server "pg-main"
    user "app1"
    secret "app1-pass"
}
"#,
        )
        .unwrap();
    }

    #[test]
    fn render_missing_secret() {
        use tempfile::TempDir;

        use crate::secret::MasterKey;

        let base = TempDir::new().unwrap();
        let key = MasterKey::generate(base.path());
        key.save().unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };

        let v = Value::parse("${secret:nope}").unwrap();
        let err = v.render(&ctx).unwrap_err();
        let RefError::At { location, inner } = err else {
            panic!("expected At wrapper, got {err:?}");
        };
        assert!(matches!(&location, Location::Token { raw } if raw == "${secret:nope}"));
        assert!(
            matches!(*inner, RefError::Secret(SecretError::NotFound { ref name }) if name == "nope")
        );
    }

    #[test]
    fn render_unit_ref() {
        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        write_db_unit(base.path());

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        let user = Value::parse("${app-db:user}").unwrap();
        assert_eq!(user.render(&ctx).unwrap(), "app1");

        let name = Value::parse("${app-db:name}").unwrap();
        assert_eq!(name.render(&ctx).unwrap(), "app-db");
    }

    #[test]
    fn render_unknown_unit() {
        let v = Value::parse("${nope:user}").unwrap();
        let err = v.render(&MainContext::default()).unwrap_err();
        let RefError::At {
            location: token_loc,
            inner: unit_layer,
        } = err
        else {
            panic!("expected At(Token), got {err:?}");
        };
        assert!(matches!(&token_loc, Location::Token { raw } if raw == "${nope:user}"));
        let RefError::At {
            location: unit_loc,
            inner: leaf,
        } = *unit_layer
        else {
            panic!("expected At(Unit) below token");
        };
        assert!(matches!(&unit_loc, Location::Unit { name } if name == "nope"));
        assert!(matches!(*leaf, RefError::UnknownUnit { ref name } if name == "nope"));
    }

    #[test]
    fn render_unknown_export() {
        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        write_db_unit(base.path());

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };

        let v = Value::parse("${app-db:unknown}").unwrap();
        let err = v.render(&ctx).unwrap_err();
        let RefError::At {
            location: token_loc,
            inner: unit_layer,
        } = err
        else {
            panic!("expected At(Token), got {err:?}");
        };
        assert!(matches!(&token_loc, Location::Token { raw } if raw == "${app-db:unknown}"));
        let RefError::At {
            location: unit_loc,
            inner: leaf,
        } = *unit_layer
        else {
            panic!("expected At(Unit) below token");
        };
        assert!(matches!(&unit_loc, Location::Unit { name } if name == "app-db"));
        assert!(matches!(*leaf, RefError::UnknownExport { ref key } if key == "unknown"));
    }

    fn parse_node(src: &str) -> Result<Value, FieldError> {
        let doc: kdl::KdlDocument = src.parse().expect("test KDL must parse");
        let node = doc
            .nodes()
            .first()
            .expect("test KDL must have at least one node");
        Value::from_kdl_node(node).map_err(|err| match err {
            NodeError::InvalidField { source, .. } => source,
            other => panic!("expected InvalidField, got {other:?}"),
        })
    }

    #[test]
    fn from_node_string() {
        let v = parse_node(r#"key "127.0.0.1""#).unwrap();
        assert_eq!(v.as_template(), "127.0.0.1");
    }

    #[test]
    fn from_node_integer() {
        let v = parse_node("key 8000").unwrap();
        assert_eq!(v.as_template(), "8000");
    }

    #[test]
    fn from_node_float() {
        let v = parse_node("key 0.5").unwrap();
        assert_eq!(v.as_template(), "0.5");
    }

    #[test]
    fn from_node_template_ref() {
        let v = parse_node(r#"key "${secret:nexus/secret-key}""#).unwrap();
        let refs: Vec<(&Ns, &str)> = v.references().collect();
        assert_eq!(refs, vec![(&Ns::Secret, "nexus/secret-key")]);
    }

    #[test]
    fn from_node_no_args() {
        let err = parse_node("key").unwrap_err();
        assert!(matches!(err, FieldError::EntryCount { .. }), "{err:?}");
    }

    #[test]
    fn from_node_too_many_args() {
        let err = parse_node(r#"key "a" "b""#).unwrap_err();
        assert!(matches!(err, FieldError::EntryCount { .. }), "{err:?}");
    }

    #[test]
    fn from_node_named_entry() {
        let err = parse_node(r#"key value="x""#).unwrap_err();
        assert!(matches!(err, FieldError::NamedEntry { .. }), "{err:?}");
    }

    #[test]
    fn from_node_child_block() {
        let err = parse_node(r#"key "x" { extra }"#).unwrap_err();
        assert!(matches!(err, FieldError::HasChildren { .. }), "{err:?}");
    }

    #[test]
    fn from_node_null() {
        let err = parse_node("key #null").unwrap_err();
        assert!(
            matches!(
                err,
                FieldError::InvalidType {
                    expected: "non-null value",
                    ..
                }
            ),
            "{err:?}",
        );
    }

    #[test]
    fn from_node_invalid_template() {
        let err = parse_node(r#"key "${secret:}""#).unwrap_err();
        assert!(
            matches!(
                err,
                FieldError::InvalidTemplate {
                    source: TemplateError::MalformedRef { .. },
                    ..
                },
            ),
            "{err:?}",
        );
    }
}
