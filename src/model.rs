use std::path::PathBuf;

use serde::Deserialize;

use crate::config::ValidateConfig;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainConfig {
    #[serde(default = "default_base_dir")]
    pub base: PathBuf,
    pub server: ServerConfig,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_server_addr")]
    pub addr: String,
    #[serde(default = "default_server_port")]
    pub port: u16,
}

impl ValidateConfig for MainConfig {
    fn default_config() -> Option<Self> {
        serde_yaml::from_str("").ok()
    }
}

fn default_base_dir() -> PathBuf {
    PathBuf::from("/opt/dpl")
}

fn default_server_addr() -> String {
    "0.0.0.0".to_string()
}

fn default_server_port() -> u16 {
    3000
}
