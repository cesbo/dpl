use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppEntityError {
    #[error("config error: {0}")]
    ConfigError(#[from] crate::error::ConfigError),
    #[error("port error: {0}")]
    PortError(io::Error),
}
