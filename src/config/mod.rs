mod resource_name;
mod secret_name;

use std::{
    fs,
    io,
    path::Path,
};

use serde::{
    Serialize,
    de::DeserializeOwned,
};
use thiserror::Error;

pub use resource_name::ResourceName;
pub use secret_name::SecretName;

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

    fn new(name: &ResourceName, kind: ConfigErrorKind) -> Self {
        Self {
            name: name.to_string(),
            kind,
        }
    }
}

pub fn load_config<T>(path: &Path, name: &ResourceName) -> Result<T, ConfigError>
where
    T: DeserializeOwned,
{
    let content = fs::read_to_string(path).map_err(|err| {
        let kind = if err.kind() == io::ErrorKind::NotFound {
            ConfigErrorKind::NotFound
        } else {
            ConfigErrorKind::Read(err)
        };
        ConfigError::new(name, kind)
    })?;
    serde_yaml::from_str(&content)
        .map_err(|err| ConfigError::new(name, ConfigErrorKind::Parse(err)))
}

pub fn save_config<T>(path: &Path, name: &ResourceName, config: &T) -> Result<(), ConfigError>
where
    T: Serialize,
{
    let yaml = serde_yaml::to_string(config)
        .map_err(|err| ConfigError::new(name, ConfigErrorKind::Serialize(err)))?;
    fs::write(path, yaml).map_err(|err| ConfigError::new(name, ConfigErrorKind::Write(err)))?;

    Ok(())
}
