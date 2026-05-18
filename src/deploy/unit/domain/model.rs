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
        let variant_span = node
            .entries()
            .first()
            .map(|e| e.span())
            .unwrap_or_else(|| node.span());

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
                field: "proxy".to_owned(),
                value: other.to_owned(),
                span: variant_span,
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

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
pub struct RouteConfig {
    /// URL path prefix (e.g. "/billing", "/billing/static")
    pub path: String,
    #[serde(flatten)]
    pub action: RouteAction,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteAction {
    ReverseProxy {
        /// Upstream URL (e.g. "http://127.0.0.1:8000")
        target: Value,
    },
    ServeFiles {
        /// Filesystem root
        root: Value,
        /// Single Page Application fallback
        #[serde(default)]
        spa: bool,
    },
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
            if !url_path(&route.path) {
                return Err(format!("invalid route path: '{}'", route.path));
            }
        }

        Ok(())
    }
}

impl DomainConfig {
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        for (index, route) in self.routes.iter().enumerate() {
            let (value, leaf) = match &route.action {
                RouteAction::ReverseProxy { target } => (target, "target"),
                RouteAction::ServeFiles { root, .. } => (root, "root"),
            };
            value
                .render(ctx)
                .map_err(|err| err.at(Location::field(format!("routes[{index}].{leaf}"))))?;
        }
        Ok(())
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

    #[test]
    fn parse_domain_config_with_custom_proxy() {
        let config: DomainConfig = serde_yaml::from_str(
            r#"
hosts:
  - example.com
  - www.example.com
proxy:
  type: custom
  header: X-Forwarded-For
  proxies:
    - 192.0.2.10
https: proxy
custom_config: |
  add_header X-Test true;
routes:
  - path: /api
    kind: reverse_proxy
    target: "${backend:url}"
  - path: /static
    kind: serve_files
    root: "/var/www/site"
    spa: true
"#,
        )
        .unwrap();

        assert_eq!(
            config.hosts,
            vec!["example.com".to_string(), "www.example.com".to_string()],
        );
        assert_eq!(
            config.proxy,
            Some(ProxyConfig::Custom {
                header: "X-Forwarded-For".into(),
                proxies: vec!["192.0.2.10".into()],
            })
        );
        assert_eq!(config.https, Some(HttpsConfig::Proxy));
        assert_eq!(config.routes.len(), 2);

        assert!(matches!(
            config.routes[0].action,
            RouteAction::ReverseProxy { .. }
        ));
        match &config.routes[1].action {
            RouteAction::ServeFiles { spa, .. } => assert!(*spa),
            _ => panic!("expected serve_files action"),
        }

        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn reject_custom_proxy_without_ip() {
        let config: DomainConfig = serde_yaml::from_str(
            r#"
hosts:
  - example.com
proxy:
  type: custom
  header: X-Forwarded-For
  proxies: []
https: acme
"#,
        )
        .unwrap();

        assert!(config.validate_config().is_err());
    }

    #[test]
    fn parse_domain_config_without_proxy_and_https() {
        let config: DomainConfig = serde_yaml::from_str(
            r#"
hosts:
  - example.com
custom_config: |
  add_header X-Domain test;
routes:
  - path: /
    kind: reverse_proxy
    target: "${backend:url}"
"#,
        )
        .unwrap();

        assert_eq!(config.proxy, None);
        assert_eq!(config.https, None);
        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn reject_missing_hosts() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
routes:
  - path: /
    kind: reverse_proxy
    target: "${backend:url}"
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn reject_empty_hosts() {
        let config: DomainConfig = serde_yaml::from_str(
            r#"
hosts: []
routes:
  - path: /
    kind: reverse_proxy
    target: "${backend:url}"
"#,
        )
        .unwrap();

        assert_eq!(
            config.validate_config(),
            Err("hosts must not be empty".into()),
        );
    }

    #[test]
    fn reject_route_with_no_action() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
hosts:
  - example.com
routes:
  - path: /api
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn reject_route_with_unknown_kind() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
hosts:
  - example.com
routes:
  - path: /api
    kind: redirect
    target: "https://example.com"
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn reject_serve_files_without_root() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
hosts:
  - example.com
routes:
  - path: /static
    kind: serve_files
    spa: true
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn reject_unknown_field_on_serve_files() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
hosts:
  - example.com
routes:
  - path: /static
    kind: serve_files
    root: "/var/www"
    bogus: true
"#,
        );

        assert!(result.is_err());
    }
}
