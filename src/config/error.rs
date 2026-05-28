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

    #[error("parse config for unit '{name}'")]
    Parse {
        name: String,
        #[source]
        source: serde_yaml::Error,
    },
}
