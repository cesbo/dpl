use std::{
    fs,
    io,
    path::{
        Path,
        PathBuf,
    },
};

use serde::Deserialize;

use crate::config::ConfigError;

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

impl MainConfig {
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let content = match fs::read_to_string(path) {
            Ok(v) => v,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::default(),
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.into(),
                    source,
                });
            }
        };

        serde_yaml::from_str(&content).map_err(|source| ConfigError::Parse {
            path: path.into(),
            source,
        })
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
