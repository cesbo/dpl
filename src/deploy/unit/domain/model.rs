use kdl::KdlNode;
use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    MainContext,
    config::{
        FromKdlNode,
        NodeError,
        ValidateConfig,
        push_field,
        reject_children,
        set_field,
    },
    deploy::env::Value,
    error::{
        Location,
        RefError,
    },
    kdl_args,
    validate::url_path,
};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    pub hosts: Vec<String>,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub https: Option<HttpsConfig>,
    #[serde(default)]
    pub custom_config: String,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProxyConfig {
    Cloudflare,
    Fastly,
    Custom {
        header: String,
        proxies: Vec<String>,
    },
}

impl ValidateConfig for ProxyConfig {
    fn validate_config(&self) -> Result<(), String> {
        match self {
            ProxyConfig::Cloudflare | ProxyConfig::Fastly => Ok(()),
            ProxyConfig::Custom { header, proxies } => {
                if header.is_empty() {
                    return Err("custom header must not be empty".into());
                }

                if proxies.is_empty() {
                    return Err("custom proxies must not be empty".into());
                }

                Ok(())
            }
        }
    }
}

impl FromKdlNode for ProxyConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let variant = kdl_args!(node, variant: String)?;

        match variant.as_str() {
            "cloudflare" => {
                reject_children(node)?;
                Ok(ProxyConfig::Cloudflare)
            }
            "fastly" => {
                reject_children(node)?;
                Ok(ProxyConfig::Fastly)
            }
            "custom" => {
                let mut header: Option<String> = None;
                let mut proxies: Vec<String> = Vec::new();

                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        let name = child.name().value();
                        match name {
                            "header" => set_field(&mut header, child)?,
                            "ip" => push_field(&mut proxies, child)?,
                            _ => {
                                return Err(NodeError::UnknownField {
                                    name: name.to_owned(),
                                    span: child.span(),
                                });
                            }
                        }
                    }
                }

                Ok(ProxyConfig::Custom {
                    header: header.ok_or(NodeError::MissingField {
                        name: "header",
                        span: node.span(),
                    })?,
                    proxies,
                })
            }
            other => Err(NodeError::UnknownVariant {
                field: node.name().value().to_owned(),
                value: other.to_owned(),
                span: node
                    .entries()
                    .first()
                    .map(|e| e.span())
                    .unwrap_or_else(|| node.span()),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HttpsConfig {
    Proxy,
    Acme,
}

impl FromKdlNode for HttpsConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let variant = kdl_args!(node, variant: String)?;

        match variant.as_str() {
            "proxy" => {
                reject_children(node)?;
                Ok(HttpsConfig::Proxy)
            }
            "acme" => {
                reject_children(node)?;
                Ok(HttpsConfig::Acme)
            }
            other => Err(NodeError::UnknownVariant {
                field: node.name().value().to_owned(),
                value: other.to_owned(),
                span: node
                    .entries()
                    .first()
                    .map(|e| e.span())
                    .unwrap_or_else(|| node.span()),
            }),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteConfig {
    ReverseProxy {
        /// URL path prefix (e.g. "/billing")
        path: String,
        /// Upstream URL (e.g. "http://127.0.0.1:8000")
        target: Value,
    },
    ServeFiles {
        /// URL path prefix (e.g. "/billing/static")
        path: String,
        /// Filesystem root
        root: Value,
        /// Single Page Application fallback
        #[serde(default)]
        spa: bool,
    },
}

impl RouteConfig {
    pub fn path(&self) -> &str {
        match self {
            RouteConfig::ReverseProxy { path, .. } | RouteConfig::ServeFiles { path, .. } => path,
        }
    }
}

impl FromKdlNode for RouteConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let (variant, path) = kdl_args!(node, variant: String, path: String)?;

        let variant_span = node
            .entries()
            .first()
            .map(|e| e.span())
            .unwrap_or_else(|| node.span());

        match variant.as_str() {
            "reverse_proxy" => {
                let mut target: Option<Value> = None;
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        let name = child.name().value();
                        match name {
                            "target" => set_value_field(&mut target, child, "target")?,
                            _ => {
                                return Err(NodeError::UnknownField {
                                    name: name.to_owned(),
                                    span: child.span(),
                                });
                            }
                        }
                    }
                }
                Ok(RouteConfig::ReverseProxy {
                    path,
                    target: target.ok_or(NodeError::MissingField {
                        name: "target",
                        span: node.span(),
                    })?,
                })
            }
            "serve_files" => {
                let mut root: Option<Value> = None;
                let mut spa = false;
                let mut spa_seen = false;
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        let name = child.name().value();
                        match name {
                            "root" => set_value_field(&mut root, child, "root")?,
                            "spa" => {
                                if spa_seen {
                                    return Err(NodeError::DuplicateField {
                                        name: "spa".to_owned(),
                                        span: child.span(),
                                    });
                                }
                                spa_seen = true;
                                kdl_args!(child)?;
                                reject_children(child)?;
                                spa = true;
                            }
                            _ => {
                                return Err(NodeError::UnknownField {
                                    name: name.to_owned(),
                                    span: child.span(),
                                });
                            }
                        }
                    }
                }
                Ok(RouteConfig::ServeFiles {
                    path,
                    root: root.ok_or(NodeError::MissingField {
                        name: "root",
                        span: node.span(),
                    })?,
                    spa,
                })
            }
            other => Err(NodeError::UnknownVariant {
                field: node.name().value().to_owned(),
                value: other.to_owned(),
                span: variant_span,
            }),
        }
    }
}

fn set_value_field(
    target: &mut Option<Value>,
    child: &KdlNode,
    name: &'static str,
) -> Result<(), NodeError> {
    if target.is_some() {
        return Err(NodeError::DuplicateField {
            name: name.to_owned(),
            span: child.span(),
        });
    }
    let value = Value::try_from(child).map_err(|source| NodeError::InvalidField {
        name: name.to_owned(),
        span: child.span(),
        source,
    })?;
    *target = Some(value);
    Ok(())
}

impl ValidateConfig for DomainConfig {
    fn validate_config(&self) -> Result<(), String> {
        if self.hosts.is_empty() {
            return Err("hosts must not be empty".into());
        }

        for host in &self.hosts {
            if host.trim().is_empty() {
                return Err("host must not be empty".into());
            }
        }

        if let Some(proxy) = &self.proxy {
            proxy.validate_config()?;
        }

        for route in &self.routes {
            if !url_path(route.path()) {
                return Err(format!("invalid route path: '{}'", route.path()));
            }
        }

        Ok(())
    }
}

impl DomainConfig {
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        for (index, route) in self.routes.iter().enumerate() {
            let (value, leaf) = match route {
                RouteConfig::ReverseProxy { target, .. } => (target, "target"),
                RouteConfig::ServeFiles { root, .. } => (root, "root"),
            };
            value
                .render(ctx)
                .map_err(|err| err.at(Location::field(format!("routes[{index}].{leaf}"))))?;
        }
        Ok(())
    }
}

impl FromKdlNode for DomainConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        kdl_args!(node)?;

        let mut hosts: Vec<String> = Vec::new();
        let mut proxy: Option<ProxyConfig> = None;
        let mut https: Option<HttpsConfig> = None;
        let mut custom_config: Option<String> = None;
        let mut routes: Vec<RouteConfig> = Vec::new();

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "host" => push_field(&mut hosts, child)?,
                    "proxy" => set_field(&mut proxy, child)?,
                    "https" => set_field(&mut https, child)?,
                    "custom-config" => set_field(&mut custom_config, child)?,
                    "route" => push_field(&mut routes, child)?,
                    _ => {
                        return Err(NodeError::UnknownField {
                            name: name.to_owned(),
                            span: child.span(),
                        });
                    }
                }
            }
        }

        Ok(DomainConfig {
            hosts,
            proxy,
            https,
            custom_config: custom_config.unwrap_or_default(),
            routes,
        })
    }
}

#[cfg(test)]
mod tests {
    use kdl::KdlDocument;

    use super::*;
    use crate::config::{
        FieldError,
        ValidateConfig,
    };

    fn parse_proxy(src: &str) -> Result<ProxyConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        ProxyConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    #[test]
    fn kdl_proxy_cloudflare_bare() {
        let cfg = parse_proxy("proxy cloudflare").unwrap();
        assert_eq!(cfg, ProxyConfig::Cloudflare);
    }

    #[test]
    fn kdl_proxy_fastly_bare() {
        let cfg = parse_proxy("proxy fastly").unwrap();
        assert_eq!(cfg, ProxyConfig::Fastly);
    }

    #[test]
    fn kdl_proxy_custom_basic() {
        let cfg = parse_proxy(
            r#"
            proxy custom {
                header "X-Forwarded-For"
                ip "192.0.2.10"
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg,
            ProxyConfig::Custom {
                header: "X-Forwarded-For".into(),
                proxies: vec!["192.0.2.10".into()],
            },
        );
    }

    #[test]
    fn kdl_proxy_custom_multiple_ips() {
        let cfg = parse_proxy(
            r#"
            proxy custom {
                header "X-Forwarded-For"
                ip "192.0.2.10"
                ip "192.0.2.11"
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg,
            ProxyConfig::Custom {
                header: "X-Forwarded-For".into(),
                proxies: vec!["192.0.2.10".into(), "192.0.2.11".into()],
            },
        );
    }

    #[test]
    fn kdl_proxy_unknown_variant() {
        let err = parse_proxy(r#"proxy "other""#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::UnknownVariant { field, value, .. }
                    if field == "proxy" && value == "other",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_proxy_missing_variant() {
        let err = parse_proxy("proxy").unwrap_err();
        assert!(
            matches!(
                err,
                NodeError::MissingArg {
                    name: "variant",
                    ..
                }
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_proxy_custom_missing_header() {
        let err = parse_proxy(
            r#"
            proxy custom {
                ip "192.0.2.10"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "header"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_proxy_custom_unknown_field() {
        let err = parse_proxy(
            r#"
            proxy custom {
                header "X-Forwarded-For"
                bogus "y"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "bogus"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_proxy_custom_duplicate_header() {
        let err = parse_proxy(
            r#"
            proxy custom {
                header "X-Forwarded-For"
                header "X-Real-IP"
                ip "192.0.2.10"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "header"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_proxy_bare_variant_with_children() {
        let err = parse_proxy(
            r#"
            proxy cloudflare {
                ip "192.0.2.10"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "ip"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_proxy_named_arg_rejected() {
        let err = parse_proxy(r#"proxy variant="custom""#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::NamedEntry { .. },
                    ..
                } if name == "variant",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_proxy_variant_not_a_string() {
        let err = parse_proxy("proxy 5").unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "variant",
            ),
            "unexpected error: {err:?}",
        );
    }

    fn parse_route(src: &str) -> Result<RouteConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        RouteConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    #[test]
    fn kdl_route_reverse_proxy() {
        let cfg = parse_route(
            r#"
            route reverse_proxy "/api" {
                target "http://127.0.0.1:8000"
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg,
            RouteConfig::ReverseProxy {
                path: "/api".into(),
                target: Value::parse("http://127.0.0.1:8000").unwrap(),
            },
        );
    }

    #[test]
    fn kdl_route_reverse_proxy_template_target() {
        let cfg = parse_route(
            r#"
            route reverse_proxy "/api" {
                target "${backend:url}"
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg,
            RouteConfig::ReverseProxy {
                path: "/api".into(),
                target: Value::parse("${backend:url}").unwrap(),
            },
        );
    }

    #[test]
    fn kdl_route_serve_files_with_spa() {
        let cfg = parse_route(
            r#"
            route serve_files "/static" {
                root "/var/www/site"
                spa
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg,
            RouteConfig::ServeFiles {
                path: "/static".into(),
                root: Value::parse("/var/www/site").unwrap(),
                spa: true,
            },
        );
    }

    #[test]
    fn kdl_route_serve_files_without_spa() {
        let cfg = parse_route(
            r#"
            route serve_files "/static" {
                root "/var/www/site"
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg,
            RouteConfig::ServeFiles {
                path: "/static".into(),
                root: Value::parse("/var/www/site").unwrap(),
                spa: false,
            },
        );
    }

    #[test]
    fn kdl_route_missing_target() {
        let err = parse_route(
            r#"
            route reverse_proxy "/api" {
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(err, NodeError::MissingField { name: "target", .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_route_missing_root() {
        let err = parse_route(
            r#"
            route serve_files "/static" {
                spa
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(err, NodeError::MissingField { name: "root", .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_route_unknown_variant() {
        let err = parse_route(r#"route redirect "/x""#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::UnknownVariant { field, value, .. }
                    if field == "route" && value == "redirect",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_route_unknown_child_on_serve_files() {
        let err = parse_route(
            r#"
            route serve_files "/static" {
                root "/var/www"
                target "http://x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "target"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_route_unknown_child_on_reverse_proxy() {
        let err = parse_route(
            r#"
            route reverse_proxy "/api" {
                target "http://x"
                bogus "y"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "bogus"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_route_duplicate_target() {
        let err = parse_route(
            r#"
            route reverse_proxy "/api" {
                target "http://a"
                target "http://b"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "target"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_route_duplicate_spa() {
        let err = parse_route(
            r#"
            route serve_files "/static" {
                root "/var/www"
                spa
                spa
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "spa"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_route_spa_with_arg_rejected() {
        let err = parse_route(
            r#"
            route serve_files "/static" {
                root "/var/www"
                spa #true
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_route_missing_path() {
        let err = parse_route(
            r#"
            route reverse_proxy {
                target "http://x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "path", .. }),
            "unexpected error: {err:?}",
        );
    }

    fn parse_https(src: &str) -> Result<HttpsConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        HttpsConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    #[test]
    fn kdl_https_proxy() {
        let cfg = parse_https("https proxy").unwrap();
        assert_eq!(cfg, HttpsConfig::Proxy);
    }

    #[test]
    fn kdl_https_acme() {
        let cfg = parse_https("https acme").unwrap();
        assert_eq!(cfg, HttpsConfig::Acme);
    }

    #[test]
    fn kdl_https_unknown_variant() {
        let err = parse_https(r#"https "other""#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::UnknownVariant { field, value, .. }
                    if field == "https" && value == "other",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_https_with_children_rejected() {
        let err = parse_https(
            r#"
            https proxy {
                bogus "y"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "bogus"),
            "unexpected error: {err:?}",
        );
    }

    fn parse_domain(src: &str) -> Result<DomainConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        DomainConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn sample_domain() -> DomainConfig {
        DomainConfig {
            hosts: vec!["example.com".into()],
            proxy: None,
            https: None,
            custom_config: String::new(),
            routes: Vec::new(),
        }
    }

    #[test]
    fn kdl_domain_full() {
        let cfg = parse_domain(
            r#"
            domain {
                host "example.com"
                host "www.example.com"
                proxy custom {
                    header "X-Forwarded-For"
                    ip "192.0.2.10"
                }
                https proxy
                custom-config "add_header X-Test true;"
                route reverse_proxy "/api" {
                    target "${backend:url}"
                }
                route serve_files "/static" {
                    root "/var/www/site"
                    spa
                }
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.hosts,
            vec!["example.com".to_string(), "www.example.com".to_string()],
        );
        assert_eq!(
            cfg.proxy,
            Some(ProxyConfig::Custom {
                header: "X-Forwarded-For".into(),
                proxies: vec!["192.0.2.10".into()],
            }),
        );
        assert_eq!(cfg.https, Some(HttpsConfig::Proxy));
        assert_eq!(cfg.custom_config, "add_header X-Test true;");
        assert_eq!(cfg.routes.len(), 2);
        assert!(matches!(cfg.routes[0], RouteConfig::ReverseProxy { .. }));
        match &cfg.routes[1] {
            RouteConfig::ServeFiles { spa, .. } => assert!(*spa),
            _ => panic!("expected serve_files action"),
        }
        assert!(cfg.validate_config().is_ok());
    }

    #[test]
    fn kdl_domain_minimal() {
        let cfg = parse_domain(
            r#"
            domain {
                host "example.com"
            }
            "#,
        )
        .unwrap();

        assert_eq!(cfg.hosts, vec!["example.com".to_string()]);
        assert_eq!(cfg.proxy, None);
        assert_eq!(cfg.https, None);
        assert_eq!(cfg.custom_config, "");
        assert!(cfg.routes.is_empty());
        assert!(cfg.validate_config().is_ok());
    }

    #[test]
    fn kdl_domain_multiple_hosts() {
        let cfg = parse_domain(
            r#"
            domain {
                host "a.example.com"
                host "b.example.com"
                host "c.example.com"
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.hosts,
            vec![
                "a.example.com".to_string(),
                "b.example.com".to_string(),
                "c.example.com".to_string(),
            ],
        );
    }

    #[test]
    fn kdl_domain_custom_config_multiline() {
        let cfg = parse_domain(
            r#"
            domain {
                host "example.com"
                custom-config """
                    add_header X-Test true;
                    add_header X-Other "ok";
                    """
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.custom_config,
            "add_header X-Test true;\nadd_header X-Other \"ok\";",
        );
    }

    #[test]
    fn kdl_domain_unknown_field() {
        let err = parse_domain(
            r#"
            domain {
                host "example.com"
                bogus "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "bogus"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_domain_duplicate_https() {
        let err = parse_domain(
            r#"
            domain {
                host "example.com"
                https proxy
                https acme
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "https"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_domain_duplicate_proxy() {
        let err = parse_domain(
            r#"
            domain {
                host "example.com"
                proxy cloudflare
                proxy fastly
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "proxy"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_domain_duplicate_custom_config() {
        let err = parse_domain(
            r#"
            domain {
                host "example.com"
                custom-config "a"
                custom-config "b"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "custom-config"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_domain_positional_arg_rejected() {
        let err = parse_domain(r#"domain "x" { host "example.com" }"#).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn domain_validate_empty_hosts() {
        let mut cfg = sample_domain();
        cfg.hosts = Vec::new();
        assert_eq!(cfg.validate_config(), Err("hosts must not be empty".into()),);
    }

    #[test]
    fn domain_validate_empty_host_string() {
        let mut cfg = sample_domain();
        cfg.hosts = vec!["   ".into()];
        assert_eq!(cfg.validate_config(), Err("host must not be empty".into()));
    }

    #[test]
    fn domain_validate_custom_proxy_no_ips() {
        let mut cfg = sample_domain();
        cfg.proxy = Some(ProxyConfig::Custom {
            header: "X-Forwarded-For".into(),
            proxies: Vec::new(),
        });
        assert!(cfg.validate_config().is_err());
    }

    #[test]
    fn domain_validate_invalid_route_path() {
        let mut cfg = sample_domain();
        cfg.routes = vec![RouteConfig::ReverseProxy {
            path: "no-leading-slash".into(),
            target: Value::parse("http://127.0.0.1:8000").unwrap(),
        }];
        assert!(cfg.validate_config().is_err());
    }
}
