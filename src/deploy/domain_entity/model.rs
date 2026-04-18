use serde::Deserialize;

use crate::{
    config::ValidateConfig,
    validate::{
        resource_name,
        url_path,
    },
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub https: Option<HttpsConfig>,
    #[serde(default)]
    pub custom_config: String,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HttpsConfig {
    Proxy,
    Acme,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RouteConfig {
    /// URL path prefix (e.g. "/billing", "/billing/static")
    pub path: String,
    /// Where the route points to
    pub target: RouteTarget,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteTarget {
    /// Proxy pass to an app entity socket
    App {
        /// Name of the app entity
        entity: String,
    },
    /// Serve static files from `{deploy_dir}/exports`
    Static {
        /// Name of the app entity that exports the files
        entity: String,
    },
}

impl ValidateConfig for DomainConfig {
    fn validate_config(&self) -> Result<(), String> {
        if let Some(proxy) = &self.proxy {
            proxy.validate_config()?;
        }

        for route in &self.routes {
            if !url_path(&route.path) {
                return Err(format!("invalid route path: '{}'", route.path));
            }

            let entity = match &route.target {
                RouteTarget::App { entity } | RouteTarget::Static { entity } => entity,
            };

            if !resource_name(entity) {
                return Err(format!(
                    "invalid entity name '{}' in route '{}'",
                    entity, route.path
                ));
            }
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
    target:
      kind: app
      entity: backend
  - path: /static
    target:
      kind: static
      entity: backend
"#,
        )
        .unwrap();

        assert_eq!(
            config.proxy,
            Some(ProxyConfig::Custom {
                header: "X-Forwarded-For".into(),
                proxies: vec!["192.0.2.10".into()],
            })
        );
        assert_eq!(config.https, Some(HttpsConfig::Proxy));
        assert_eq!(config.routes.len(), 2);
        assert_eq!(
            config.routes[0].target,
            RouteTarget::App {
                entity: "backend".into()
            }
        );
        assert_eq!(
            config.routes[1].target,
            RouteTarget::Static {
                entity: "backend".into()
            }
        );
        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn reject_custom_proxy_without_ip() {
        let config: DomainConfig = serde_yaml::from_str(
            r#"
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
custom_config: |
  add_header X-Domain test;
routes:
  - path: /
    target:
      kind: app
      entity: backend
"#,
        )
        .unwrap();

        assert_eq!(config.proxy, None);
        assert_eq!(config.https, None);
        assert!(config.validate_config().is_ok());
    }
}
