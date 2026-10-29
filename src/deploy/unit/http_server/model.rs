use std::{
    fmt,
    net::{
        IpAddr,
        SocketAddr,
    },
    str::FromStr,
};

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

/// Host publish for an http-server. nginx always listens on port 80 inside the
/// container; this only describes the host side of `podman --publish`.
///
/// `listen` left out means port 80 on every interface. A socket address
/// publishes on that host address only. `listen: false` (or `null`) publishes
/// nothing. An explicit `http_port` together with `listen` is rejected;
/// `http_port` remains only so older configs still parse.
#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(try_from = "HttpServerConfigRaw")]
pub struct HttpServerConfig {
    pub image: String,

    /// All-interfaces host port. `Disabled` when the host publishes nothing
    /// that way: explicit `false`/`null`, or `listen` is set instead.
    pub http_port: HttpPort,

    /// Publish on one host address. `None` when `http_port` is the publish.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen: Option<HttpListen>,

    pub access_log: HttpAccessLogConfig,
}

/// Host address and port to publish nginx's container port 80 on.
///
/// Written as one socket address: `"[2001:db8::1]:80"` or `"192.0.2.1:8080"`.
/// The address is a host IP, not an interface name. Binding that address
/// accepts connections only to it, which is how a port is limited to the
/// interface that owns it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HttpListen {
    pub address: IpAddr,
    pub port: u16,
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
                "must be a port number or false",
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

/// `http_port` as written. Absent is not the same as `false`: the default port
/// applies only when both this and `listen` are absent.
#[derive(Debug, Default)]
enum HttpPortField {
    #[default]
    Absent,
    Present(HttpPort),
}

impl<'de> Deserialize<'de> for HttpPortField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        HttpPort::deserialize(deserializer).map(Self::Present)
    }
}

/// `listen` as written. Absent is not `false`: only an absent field keeps the
/// default publish.
#[derive(Debug, Default)]
enum ListenField {
    #[default]
    Absent,
    Disabled,
    Address(HttpListen),
}

impl<'de> Deserialize<'de> for ListenField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ListenVisitor;

        impl<'de> de::Visitor<'de> for ListenVisitor {
            type Value = ListenField;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a socket address or false")
            }

            fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
                if value {
                    Err(E::custom("must be an address or false"))
                } else {
                    Ok(ListenField::Disabled)
                }
            }

            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(ListenField::Disabled)
            }

            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(ListenField::Disabled)
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
                value.parse().map(ListenField::Address).map_err(E::custom)
            }
        }

        deserializer.deserialize_any(ListenVisitor)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpServerConfigRaw {
    #[serde(default = "default_image")]
    image: String,

    #[serde(default)]
    http_port: HttpPortField,

    #[serde(default)]
    listen: ListenField,

    #[serde(default)]
    access_log: HttpAccessLogConfig,
}

impl TryFrom<HttpServerConfigRaw> for HttpServerConfig {
    type Error = String;

    fn try_from(raw: HttpServerConfigRaw) -> Result<Self, Self::Error> {
        let (http_port, listen) = match (raw.http_port, raw.listen) {
            (HttpPortField::Present(HttpPort::Port(_)), ListenField::Address(_) | ListenField::Disabled) => {
                return Err(
                    "http_port and listen are mutually exclusive; omit http_port".to_string(),
                );
            }
            (
                HttpPortField::Absent | HttpPortField::Present(HttpPort::Disabled),
                ListenField::Address(listen),
            ) => (HttpPort::Disabled, Some(listen)),
            (_, ListenField::Disabled) => (HttpPort::Disabled, None),
            (HttpPortField::Absent, ListenField::Absent) => (HttpPort::Port(DEFAULT_HTTP_PORT), None),
            (HttpPortField::Present(port), ListenField::Absent) => (port, None),
        };

        Ok(Self {
            image: raw.image,
            http_port,
            listen,
            access_log: raw.access_log,
        })
    }
}

impl HttpListen {
    /// `"[2001:db8::1]:80"` or `"192.0.2.1:8080"`, the form podman publishes.
    fn format(&self) -> String {
        SocketAddr::new(self.address, self.port).to_string()
    }
}

impl FromStr for HttpListen {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.contains('%') {
            return Err(
                "must not include an interface zone; podman binds an address, not an interface"
                    .to_string(),
            );
        }
        let addr: SocketAddr = value.parse().map_err(|_| {
            format!("invalid address `{value}`; expected `[2001:db8::1]:80` or `192.0.2.1:8080`")
        })?;
        if addr.port() == 0 {
            return Err("port must not be 0".to_string());
        }
        if !is_bindable(addr.ip()) {
            return Err("address must be a unicast IP".to_string());
        }
        Ok(Self {
            address: addr.ip(),
            port: addr.port(),
        })
    }
}

impl<'de> Deserialize<'de> for HttpListen {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
    }
}

impl Serialize for HttpListen {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.format())
    }
}

fn is_bindable(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(v4) => !v4.is_multicast() && !v4.is_broadcast(),
        IpAddr::V6(v6) => !v6.is_multicast(),
    }
}

impl HttpServerConfig {
    pub const KIND: &'static str = "http-server";

    /// True when the unit publishes container port 80 on the host, either on
    /// every interface (`http_port`) or on one address (`listen`).
    pub fn publishes_host_port(&self) -> bool {
        self.listen.is_some() || matches!(self.http_port, HttpPort::Port(_))
    }

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
        assert_eq!(config.http_port, HttpPort::Port(DEFAULT_HTTP_PORT));
        assert!(config.listen.is_none());
        assert!(config.publishes_host_port());
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
access_log:
  max_size_mb: 512
  max_files: 3
"#,
        )
        .unwrap();
        assert_eq!(config.image, "docker.io/library/nginx:1.27");
        assert_eq!(config.http_port, HttpPort::Port(8080));
        assert!(config.listen.is_none());
        assert_eq!(config.access_log.max_size_mb, 512);
        assert_eq!(config.access_log.max_files, 3);
        assert_eq!(config.access_log.max_size_bytes(), 512 * 1024 * 1024);
    }

    #[test]
    fn reject_https_port() {
        let err = serde_yaml::from_str::<HttpServerConfig>("https_port: 443\n").unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_http_port_false_as_disabled() {
        let config: HttpServerConfig = serde_yaml::from_str("http_port: false\n").unwrap();
        assert_eq!(config.http_port, HttpPort::Disabled);
        assert!(!config.publishes_host_port());
    }

    #[test]
    fn parse_http_port_null_as_disabled() {
        let config: HttpServerConfig = serde_yaml::from_str("http_port: null\n").unwrap();
        assert_eq!(config.http_port, HttpPort::Disabled);
        assert!(!config.publishes_host_port());
    }

    #[test]
    fn reject_http_port_true() {
        let err = serde_yaml::from_str::<HttpServerConfig>("http_port: true\n").unwrap_err();
        assert!(
            err.to_string().contains("must be a port number or false"),
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

    #[test]
    fn listen_replaces_the_default_publish() {
        let config: HttpServerConfig =
            serde_yaml::from_str("listen: \"[2001:db8::1]:8080\"\n").unwrap();
        assert_eq!(config.http_port, HttpPort::Disabled);
        assert_eq!(
            config.listen,
            Some(HttpListen {
                address: "2001:db8::1".parse().unwrap(),
                port: 8080,
            })
        );
        assert_eq!(
            config.listen.as_ref().unwrap().format(),
            "[2001:db8::1]:8080"
        );
        assert!(config.publishes_host_port());
    }

    #[test]
    fn listen_false_publishes_nothing() {
        let config: HttpServerConfig = serde_yaml::from_str("listen: false\n").unwrap();
        assert_eq!(config.http_port, HttpPort::Disabled);
        assert!(config.listen.is_none());
        assert!(!config.publishes_host_port());

        let config: HttpServerConfig = serde_yaml::from_str("listen: null\n").unwrap();
        assert!(!config.publishes_host_port());
    }

    #[test]
    fn reject_listen_true() {
        let err = serde_yaml::from_str::<HttpServerConfig>("listen: true\n").unwrap_err();
        assert!(
            err.to_string().contains("must be an address or false"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn listen_with_http_port_false_publishes_only_the_address() {
        let config: HttpServerConfig =
            serde_yaml::from_str("http_port: false\nlisten: \"192.0.2.10:80\"\n").unwrap();
        assert_eq!(config.http_port, HttpPort::Disabled);
        assert_eq!(
            config.listen,
            Some(HttpListen {
                address: "192.0.2.10".parse().unwrap(),
                port: 80,
            })
        );
    }

    #[test]
    fn listen_round_trip() {
        let config: HttpServerConfig =
            serde_yaml::from_str("listen: \"[2001:db8::1]:8080\"\n").unwrap();
        let yaml = serde_yaml::to_string(&config).unwrap();
        assert!(yaml.contains("[2001:db8::1]:8080"), "{yaml}");
        let again: HttpServerConfig = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(config, again);
    }

    #[test]
    fn reject_listen_with_an_explicit_port() {
        let err = serde_yaml::from_str::<HttpServerConfig>(
            "http_port: 80\nlisten: \"[2001:db8::1]:80\"\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("mutually exclusive"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_listen_port_zero() {
        let err =
            serde_yaml::from_str::<HttpServerConfig>("listen: \"[2001:db8::1]:0\"\n").unwrap_err();
        assert!(
            err.to_string().contains("port must not be 0"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_listen_interface_name() {
        let err = serde_yaml::from_str::<HttpServerConfig>("listen: eth0\n").unwrap_err();
        assert!(
            err.to_string().contains("invalid address"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_listen_without_brackets() {
        let err =
            serde_yaml::from_str::<HttpServerConfig>("listen: \"2001:db8::1:80\"\n").unwrap_err();
        assert!(
            err.to_string().contains("expected `[2001:db8::1]:80`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_listen_zone() {
        let err = serde_yaml::from_str::<HttpServerConfig>("listen: \"[fe80::1%eth0]:80\"\n")
            .unwrap_err();
        assert!(
            err.to_string().contains("interface zone"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_listen_multicast() {
        let err =
            serde_yaml::from_str::<HttpServerConfig>("listen: \"[ff02::1]:80\"\n").unwrap_err();
        assert!(
            err.to_string().contains("unicast"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn reject_listen_mapping() {
        let err = serde_yaml::from_str::<HttpServerConfig>(
            "listen:\n  address: \"2001:db8::1\"\n  port: 80\n",
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("a socket address or false"),
            "unexpected error: {err}"
        );
    }
}
