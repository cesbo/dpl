use kdl::{
    KdlDocument,
    KdlEntry,
    KdlNode,
};

use super::RouteLocation;
use crate::{
    MainContext,
    config::{
        FromKdlNode,
        HostName,
        NodeError,
        NodeList,
        NodeValue,
        reject_children,
        string_node,
    },
    deploy::env::Value,
    error::{
        Location,
        RefError,
    },
    kdl_args,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DomainConfig {
    pub hosts: Vec<HostName>,
    pub proxy: Option<ProxyConfig>,
    pub custom_config: String,
    pub routes: Vec<RouteConfig>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProxyConfig {
    Cloudflare,
    Fastly,
    Custom {
        header: String,
        proxies: Vec<String>,
    },
}

impl ProxyConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("proxy");
        match self {
            ProxyConfig::Cloudflare => {
                node.entries_mut()
                    .push(KdlEntry::new("cloudflare".to_owned()));
            }
            ProxyConfig::Fastly => {
                node.entries_mut().push(KdlEntry::new("fastly".to_owned()));
            }
            ProxyConfig::Custom { header, proxies } => {
                node.entries_mut().push(KdlEntry::new("custom".to_owned()));
                let mut children = KdlDocument::new();
                children.nodes_mut().push(string_node("header", header));
                for ip in proxies {
                    children.nodes_mut().push(string_node("ip", ip));
                }
                node.set_children(children);
            }
        }
        node
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
                let mut header = NodeValue::<String>::new("header");
                let mut proxies = NodeList::<String>::new("ip");

                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        let name = child.name().value();
                        match name {
                            "header" => header.set(child)?,
                            "ip" => proxies.push(child)?,
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
                    header: header.take_required(node)?,
                    proxies: proxies.take_required(node)?,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RouteConfig {
    ReverseProxy {
        /// URL location prefix (e.g. "/billing")
        location: RouteLocation,
        /// Upstream URL (e.g. "http://127.0.0.1:8000")
        target: Value,
    },
    ServeFiles {
        /// URL location prefix (e.g. "/billing/static")
        location: RouteLocation,
        /// Filesystem root
        root: Value,
        /// Single Page Application fallback
        spa: bool,
    },
}

impl RouteConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("route");
        match self {
            RouteConfig::ReverseProxy { location, target } => {
                node.entries_mut()
                    .push(KdlEntry::new("reverse_proxy".to_owned()));
                node.entries_mut().push(KdlEntry::from(location));
                let mut children = KdlDocument::new();
                children
                    .nodes_mut()
                    .push(string_node("target", &target.as_template()));
                node.set_children(children);
            }
            RouteConfig::ServeFiles {
                location,
                root,
                spa,
            } => {
                node.entries_mut()
                    .push(KdlEntry::new("serve_files".to_owned()));
                node.entries_mut().push(KdlEntry::from(location));
                let mut children = KdlDocument::new();
                children
                    .nodes_mut()
                    .push(string_node("root", &root.as_template()));
                if *spa {
                    children.nodes_mut().push(KdlNode::new("spa"));
                }
                node.set_children(children);
            }
        }
        node
    }
}

impl FromKdlNode for RouteConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let (variant, location) = kdl_args!(node, variant: String, location: RouteLocation)?;

        let variant_span = node
            .entries()
            .first()
            .map(|e| e.span())
            .unwrap_or_else(|| node.span());

        match variant.as_str() {
            "reverse_proxy" => {
                let mut target = NodeValue::<Value>::new("target");

                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        let name = child.name().value();
                        match name {
                            "target" => target.set(child)?,
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
                    location,
                    target: target.take_required(node)?,
                })
            }
            "serve_files" => {
                let mut root = NodeValue::<Value>::new("root");
                let mut spa = false;
                let mut spa_seen = false;

                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        let name = child.name().value();
                        match name {
                            "root" => root.set(child)?,
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
                    location,
                    root: root.take_required(node)?,
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

impl DomainConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("domain");
        let mut children = KdlDocument::new();
        for host in &self.hosts {
            children
                .nodes_mut()
                .push(string_node("host", host.as_str()));
        }
        if let Some(proxy) = &self.proxy {
            children.nodes_mut().push(proxy.to_kdl_node());
        }
        if !self.custom_config.is_empty() {
            children
                .nodes_mut()
                .push(string_node("custom-config", &self.custom_config));
        }
        for route in &self.routes {
            children.nodes_mut().push(route.to_kdl_node());
        }
        node.set_children(children);
        node
    }

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

        let mut hosts = NodeList::<HostName>::new("host");
        let mut proxy = NodeValue::<ProxyConfig>::new("proxy");
        let mut custom_config = NodeValue::<String>::new("custom-config");
        let mut routes = NodeList::<RouteConfig>::new("route");

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "host" => hosts.push(child)?,
                    "proxy" => proxy.set(child)?,
                    "custom-config" => custom_config.set(child)?,
                    "route" => routes.push(child)?,
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
            hosts: hosts.take_required(node)?,
            proxy: proxy.take_optional(),
            custom_config: custom_config.take_optional().unwrap_or_default(),
            routes: routes.take_required(node)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use kdl::KdlDocument;

    use super::*;
    use crate::config::FieldError;

    fn parse_proxy(src: &str) -> Result<ProxyConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        ProxyConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    #[test]
    fn proxy_cloudflare_bare() {
        let cfg = parse_proxy("proxy cloudflare").unwrap();
        assert_eq!(cfg, ProxyConfig::Cloudflare);
    }

    #[test]
    fn proxy_fastly_bare() {
        let cfg = parse_proxy("proxy fastly").unwrap();
        assert_eq!(cfg, ProxyConfig::Fastly);
    }

    #[test]
    fn proxy_custom_basic() {
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
    fn proxy_custom_multiple_ips() {
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
    fn proxy_unknown_variant() {
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
    fn proxy_missing_variant() {
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
    fn proxy_custom_missing_header() {
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
    fn proxy_custom_missing_ip() {
        let err = parse_proxy(
            r#"
            proxy custom {
                header "X-Forwarded-For"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "ip"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn proxy_custom_unknown_field() {
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
    fn proxy_custom_duplicate_header() {
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
    fn proxy_bare_variant_with_children() {
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
    fn proxy_named_arg_rejected() {
        let err = parse_proxy(r#"proxy variant="custom""#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::NamedEntry { .. },
                    ..
                } if name == "proxy",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn proxy_variant_not_a_string() {
        let err = parse_proxy("proxy 5").unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "proxy",
            ),
            "unexpected error: {err:?}",
        );
    }

    fn parse_route(src: &str) -> Result<RouteConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        RouteConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    #[test]
    fn route_reverse_proxy() {
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
                location: RouteLocation::new("/api").unwrap(),
                target: Value::parse("http://127.0.0.1:8000").unwrap(),
            },
        );
    }

    #[test]
    fn route_reverse_proxy_template_target() {
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
                location: RouteLocation::new("/api").unwrap(),
                target: Value::parse("${backend:url}").unwrap(),
            },
        );
    }

    #[test]
    fn route_serve_files_with_spa() {
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
                location: RouteLocation::new("/static").unwrap(),
                root: Value::parse("/var/www/site").unwrap(),
                spa: true,
            },
        );
    }

    #[test]
    fn route_serve_files_without_spa() {
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
                location: RouteLocation::new("/static").unwrap(),
                root: Value::parse("/var/www/site").unwrap(),
                spa: false,
            },
        );
    }

    #[test]
    fn route_missing_target() {
        let err = parse_route(
            r#"
            route reverse_proxy "/api" {
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if name == "target"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn route_missing_root() {
        let err = parse_route(
            r#"
            route serve_files "/static" {
                spa
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if name == "root"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn route_unknown_variant() {
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
    fn route_unknown_child_on_serve_files() {
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
    fn route_unknown_child_on_reverse_proxy() {
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
    fn route_duplicate_target() {
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
    fn route_duplicate_spa() {
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
    fn route_spa_with_arg_rejected() {
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
    fn route_missing_location() {
        let err = parse_route(
            r#"
            route reverse_proxy {
                target "http://x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                err,
                NodeError::MissingArg {
                    name: "location",
                    ..
                }
            ),
            "unexpected error: {err:?}",
        );
    }

    fn parse_domain(src: &str) -> Result<DomainConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        DomainConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn sample_domain() -> DomainConfig {
        DomainConfig {
            hosts: vec![HostName::new("example.com").unwrap()],
            proxy: None,
            custom_config: String::new(),
            routes: vec![RouteConfig::ReverseProxy {
                location: RouteLocation::new("/").unwrap(),
                target: Value::parse("http://127.0.0.1:8000").unwrap(),
            }],
        }
    }

    #[test]
    fn domain_full() {
        let cfg = parse_domain(
            r#"
            domain {
                host "example.com"
                host "www.example.com"
                proxy custom {
                    header "X-Forwarded-For"
                    ip "192.0.2.10"
                }
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
            vec![
                HostName::new("example.com").unwrap(),
                HostName::new("www.example.com").unwrap(),
            ],
        );
        assert_eq!(
            cfg.proxy,
            Some(ProxyConfig::Custom {
                header: "X-Forwarded-For".into(),
                proxies: vec!["192.0.2.10".into()],
            }),
        );
        assert_eq!(cfg.custom_config, "add_header X-Test true;");
        assert_eq!(cfg.routes.len(), 2);
        assert!(matches!(cfg.routes[0], RouteConfig::ReverseProxy { .. }));
        match &cfg.routes[1] {
            RouteConfig::ServeFiles { spa, .. } => assert!(*spa),
            _ => panic!("expected serve_files action"),
        }
    }

    #[test]
    fn domain_minimal() {
        let cfg = parse_domain(
            r#"
            domain {
                host "example.com"
                route reverse_proxy "/" {
                    target "http://127.0.0.1:8000"
                }
            }
            "#,
        )
        .unwrap();

        assert_eq!(cfg.hosts, vec![HostName::new("example.com").unwrap()]);
        assert_eq!(cfg.proxy, None);
        assert_eq!(cfg.custom_config, "");
        assert_eq!(cfg.routes.len(), 1);
    }

    #[test]
    fn domain_multiple_hosts() {
        let cfg = parse_domain(
            r#"
            domain {
                host "a.example.com"
                host "b.example.com"
                host "c.example.com"
                route reverse_proxy "/" {
                    target "http://127.0.0.1:8000"
                }
            }
            "#,
        )
        .unwrap();

        assert_eq!(
            cfg.hosts,
            vec![
                HostName::new("a.example.com").unwrap(),
                HostName::new("b.example.com").unwrap(),
                HostName::new("c.example.com").unwrap(),
            ],
        );
    }

    #[test]
    fn domain_custom_config_multiline() {
        let cfg = parse_domain(
            r#"
            domain {
                host "example.com"
                custom-config """
                    add_header X-Test true;
                    add_header X-Other "ok";
                    """
                route reverse_proxy "/" {
                    target "http://127.0.0.1:8000"
                }
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
    fn domain_unknown_field() {
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
    fn domain_duplicate_proxy() {
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
    fn domain_duplicate_custom_config() {
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
    fn domain_positional_arg_rejected() {
        let err = parse_domain(r#"domain "x" { host "example.com" }"#).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn domain_rejects_empty_hosts() {
        let err = parse_domain("domain {}").unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if name == "host"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn domain_rejects_invalid_host() {
        let err = parse_domain(
            r#"
            domain {
                host "Bad_Host"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "host",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn route_rejects_invalid_location() {
        let err = parse_route(
            r#"
            route reverse_proxy "no-leading-slash" {
                target "http://127.0.0.1:8000"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "route",
            ),
            "unexpected error: {err:?}",
        );
    }
}
