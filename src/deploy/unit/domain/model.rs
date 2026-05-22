use serde::{
    Deserialize,
    Serialize,
};

use super::route_location::RouteLocation;
use crate::{
    MainContext,
    deploy::env::Value,
    error::{
        Location,
        RefError,
    },
};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    pub hosts: Vec<String>,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
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

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
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
        /// Single Page Application
        #[serde(default)]
        spa: bool,
    },
}

impl DomainConfig {
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        for (index, route) in self.routes.iter().enumerate() {
            let (value, leaf) = match &route {
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

#[cfg(test)]
mod tests {
    use super::*;

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
custom_config: |
  add_header X-Test true;
routes:
  - location: /api
    kind: reverse_proxy
    target: "${backend:url}"
  - location: /static
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
        assert_eq!(config.routes.len(), 2);

        assert!(matches!(config.routes[0], RouteConfig::ReverseProxy { .. }));
        match &config.routes[1] {
            RouteConfig::ServeFiles { spa, .. } => assert!(*spa),
            _ => panic!("expected serve_files action"),
        }
    }

    #[test]
    fn reject_custom_proxy_without_ip() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
hosts:
  - example.com
proxy:
  type: custom
  header: X-Forwarded-For
  proxies: []
https: acme
"#,
        );

        assert!(result.is_err());
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
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
hosts: []
routes:
  - path: /
    kind: reverse_proxy
    target: "${backend:url}"
"#,
        );

        assert!(result.is_err());
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
