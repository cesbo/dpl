use serde::Deserialize;

use crate::{
    config::ValidateConfig,
    validate::resource_name,
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
        proxy_header: String,
        proxy_ip: Vec<String>,
    },
}

impl ValidateConfig for ProxyConfig {
    fn validate_config(&self) -> Result<(), String> {
        match self {
            ProxyConfig::Cloudflare | ProxyConfig::Fastly => Ok(()),
            ProxyConfig::Custom {
                proxy_header,
                proxy_ip,
            } => {
                if proxy_header.is_empty() {
                    return Err("custom proxy_header must not be empty".into());
                }

                if proxy_ip.is_empty() {
                    return Err("custom proxy_ip must not be empty".into());
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
    pub app: String,
    pub resource: String,
}

impl ValidateConfig for DomainConfig {
    fn validate_config(&self) -> Result<(), String> {
        if let Some(proxy) = &self.proxy {
            proxy.validate_config()?;
        }

        for route in &self.routes {
            if !resource_name(&route.app) {
                return Err(format!("invalid route app name: '{}'", route.app));
            }

            if route.resource.is_empty() {
                return Err(format!(
                    "route resource must not be empty for app '{}'",
                    route.app
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
  proxy_header: X-Forwarded-For
  proxy_ip:
    - 192.0.2.10
https: proxy
custom_config: |
  add_header X-Test true;
routes:
  - app: backend
    resource: static
"#,
        )
        .unwrap();

        assert_eq!(
            config.proxy,
            Some(ProxyConfig::Custom {
                proxy_header: "X-Forwarded-For".into(),
                proxy_ip: vec!["192.0.2.10".into()],
            })
        );
        assert_eq!(config.https, Some(HttpsConfig::Proxy));
        assert_eq!(config.routes.len(), 1);
        assert!(config.validate_config().is_ok());
    }

    #[test]
    fn reject_custom_proxy_without_ip() {
        let config: DomainConfig = serde_yaml::from_str(
            r#"
proxy:
  type: custom
  proxy_header: X-Forwarded-For
  proxy_ip: []
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
  - app: backend
    resource: static
"#,
        )
        .unwrap();

        assert_eq!(config.proxy, None);
        assert_eq!(config.https, None);
        assert!(config.validate_config().is_ok());
    }
}
