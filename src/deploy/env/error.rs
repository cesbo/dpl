use thiserror::Error;

use crate::{
    config::ConfigError,
    secret::SecretError,
};

#[derive(Debug, Error)]
pub enum EnvError {
    #[error("secret")]
    Secret(#[from] SecretError),

    #[error("secret '{name}' does not exist")]
    MissingSecret { name: String },

    #[error("unit '{name}' not found")]
    UnitNotFound { name: String },

    #[error("unit '{unit}' (type {kind}) does not export '{key}'")]
    UnknownExport {
        unit: String,
        kind: &'static str,
        key: String,
    },

    #[error("load unit '{name}'")]
    UnitConfig {
        name: String,
        #[source]
        source: ConfigError,
    },
}
