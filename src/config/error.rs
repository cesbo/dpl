use std::io;

use thiserror::Error;

use super::ResourceName;

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
