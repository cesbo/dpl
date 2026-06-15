use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
    de,
};

use crate::{
    MainContext,
    reference::ReferenceError,
};

const DEFAULT_IMAGE: &str = "docker.io/library/nginx:stable";
const DEFAULT_HTTP_PORT: u16 = 80;
const DEFAULT_ACCESS_LOG_MAX_SIZE_MB: u64 = 200;
const DEFAULT_ACCESS_LOG_MAX_FILES: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HttpServerConfig {
    #[serde(default = "default_image")]
    pub image: String,

    #[serde(default = "default_http_port")]
    pub http_port: u16,

    #[serde(default)]
    pub https_port: HttpPort,

    #[serde(default)]
    pub access_log: HttpAccessLogConfig,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HttpAccessLogConfig {
    #[serde(default = "default_access_log_max_size_mb")]
    pub max_size_mb: u64,

    #[serde(default = "default_access_log_max_files")]
    pub max_files: u32,
}

#[derive(Default, Clone, Copy, Debug, Eq, PartialEq)]
pub enum HttpPort {
    #[default]
    Disabled,
    Port(u16),
}

fn default_image() -> String {
    DEFAULT_IMAGE.to_string()
}

fn default_http_port() -> u16 {
    DEFAULT_HTTP_PORT
}

fn default_access_log_max_size_mb() -> u64 {
    DEFAULT_ACCESS_LOG_MAX_SIZE_MB
}

fn default_access_log_max_files() -> u32 {
    DEFAULT_ACCESS_LOG_MAX_FILES
}

impl<'de> Deserialize<'de> for HttpPort {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ConfigValue {
            Port(u16),
            Disabled(bool),
        }

        match Option::<ConfigValue>::deserialize(deserializer)? {
            None => Ok(Self::Disabled),
            Some(ConfigValue::Port(port)) => Ok(Self::Port(port)),
            Some(ConfigValue::Disabled(false)) => Ok(Self::Disabled),
            Some(ConfigValue::Disabled(true)) => Err(de::Error::custom(
                "https_port must be a port number or false",
            )),
        }
    }
}

impl Serialize for HttpPort {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Disabled => serializer.serialize_bool(false),
            Self::Port(port) => serializer.serialize_u16(*port),
        }
    }
}

impl HttpServerConfig {
    pub const KIND: &'static str = "http-server";

    pub fn validate_references(&self, _ctx: &MainContext) -> Result<(), ReferenceError> {
        Ok(())
    }
}

impl Default for HttpAccessLogConfig {
    fn default() -> Self {
        Self {
            max_size_mb: DEFAULT_ACCESS_LOG_MAX_SIZE_MB,
            max_files: DEFAULT_ACCESS_LOG_MAX_FILES,
        }
    }
}

impl HttpAccessLogConfig {
    pub fn max_size_bytes(&self) -> u64 {
        self.max_size_mb * 1024 * 1024
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config_uses_defaults() {
        let config: HttpServerConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(config.image, DEFAULT_IMAGE);
        assert_eq!(config.http_port, DEFAULT_HTTP_PORT);
        assert_eq!(config.https_port, HttpPort::Disabled);
        assert_eq!(
            config.access_log.max_size_mb,
            DEFAULT_ACCESS_LOG_MAX_SIZE_MB
        );
        assert_eq!(config.access_log.max_files, DEFAULT_ACCESS_LOG_MAX_FILES);
        assert_eq!(config.access_log.max_size_bytes(), 200 * 1024 * 1024);
    }

    #[test]
    fn parse_full_config() {
        let config: HttpServerConfig = serde_yaml::from_str(
            r#"
image: docker.io/library/nginx:1.27
http_port: 8080
https_port: 8443
access_log:
  max_size_mb: 512
  max_files: 3
"#,
        )
        .unwrap();
        assert_eq!(config.image, "docker.io/library/nginx:1.27");
        assert_eq!(config.http_port, 8080);
        assert_eq!(config.https_port, HttpPort::Port(8443));
        assert_eq!(config.access_log.max_size_mb, 512);
        assert_eq!(config.access_log.max_files, 3);
        assert_eq!(config.access_log.max_size_bytes(), 512 * 1024 * 1024);
    }

    #[test]
    fn parse_https_port_false_as_disabled() {
        let config: HttpServerConfig = serde_yaml::from_str("https_port: false\n").unwrap();
        assert_eq!(config.https_port, HttpPort::Disabled);
    }

    #[test]
    fn parse_https_port_null_as_disabled() {
        let config: HttpServerConfig = serde_yaml::from_str("https_port: null\n").unwrap();
        assert_eq!(config.https_port, HttpPort::Disabled);
    }

    #[test]
    fn reject_https_port_true() {
        let err = serde_yaml::from_str::<HttpServerConfig>("https_port: true\n").unwrap_err();
        assert!(
            err.to_string()
                .contains("https_port must be a port number or false"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_partial_access_log_config_uses_defaults() {
        let config: HttpServerConfig = serde_yaml::from_str(
            r#"
access_log:
  max_files: 3
"#,
        )
        .unwrap();

        assert_eq!(
            config.access_log.max_size_mb,
            DEFAULT_ACCESS_LOG_MAX_SIZE_MB
        );
        assert_eq!(config.access_log.max_files, 3);
    }

    #[test]
    fn reject_unknown_field() {
        let err = serde_yaml::from_str::<HttpServerConfig>("port: 80\n").unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }
}
