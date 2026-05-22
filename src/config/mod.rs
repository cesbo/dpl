mod resource_name;
mod secret_name;

use std::io;

pub use resource_name::ResourceName;
pub use secret_name::SecretName;
use thiserror::Error;

#[derive(Debug, Error)]
#[error("unit '{name}' config")]
pub struct ConfigError {
    pub name: String,
    #[source]
    pub kind: ConfigErrorKind,
}

#[derive(Debug, Error)]
pub enum ConfigErrorKind {
    #[error("not found")]
    NotFound,

    #[error("read")]
    Read(#[source] io::Error),

    #[error("write")]
    Write(#[source] io::Error),

    #[error("parse")]
    Parse(#[source] serde_yaml::Error),

    #[error("serialize")]
    Serialize(#[source] serde_yaml::Error),
}

impl ConfigError {
    pub fn is_not_found(&self) -> bool {
        matches!(self.kind, ConfigErrorKind::NotFound)
    }

    pub fn new(name: &ResourceName, kind: ConfigErrorKind) -> Self {
        Self {
            name: name.to_string(),
            kind,
        }
    }
}
