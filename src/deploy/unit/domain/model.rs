use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    MainContext,
    config::ValidateConfig,
    deploy::env::Value,
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
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), String> {
        for route in &self.routes {
            let value = match &route.action {
                RouteAction::ReverseProxy { target } => target,
                RouteAction::ServeFiles { root, .. } => root,
            };
            value
                .validate_references(ctx)
                .map_err(|err| format!("route '{}': {err}", route.path))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ValidateConfig;

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
