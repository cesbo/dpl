use std::path::Path;

use serde::{
    Deserialize,
    Serialize,
};

use crate::config::{
    ConfigError,
    ValidateConfig,
    load_config,
};

#[derive(Default, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct MainConfig {
    pub server: ServerConfig,
}

impl MainConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        load_config(path)
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields, default)]
pub struct ServerConfig {
    pub addr: String,
    pub port: u16,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: "0.0.0.0".to_string(),
            port: 3000,
        }
    }
}

impl ValidateConfig for MainConfig {
    fn default_config() -> Option<Self> {
        Some(Self::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn main_config_addr_only_keeps_default_port() {
        let config: MainConfig = serde_yaml::from_str("server:\n  addr: 1.2.3.4\n").unwrap();
        assert_eq!(config.server.addr, "1.2.3.4");
        assert_eq!(config.server.port, 3000);
    }

    #[test]
    fn main_config_default_config() {
        let config = MainConfig::default_config().expect("default_config should return Some");
        assert_eq!(config.server.addr, "0.0.0.0");
        assert_eq!(config.server.port, 3000);
    }
}
