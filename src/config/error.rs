use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("unit '{name}' not found")]
    NotFound { name: String },

    #[error("read config for unit '{name}'")]
    Read {
        name: String,
        #[source]
        source: io::Error,
    },

    #[error("write config for unit '{name}'")]
    Write {
        name: String,
        #[source]
        source: io::Error,
    },

    #[error("parse config for unit '{name}'")]
    Parse {
        name: String,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("serialize config for unit '{name}'")]
    Serialize {
        name: String,
        #[source]
        source: serde_yaml::Error,
    },
}

impl ConfigError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, ConfigError::NotFound { .. })
    }
}
