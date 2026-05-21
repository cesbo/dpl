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
pub enum ConfigError {
    #[error("read config")]
    Read(#[source] io::Error),

    #[error("write config")]
    Write(#[source] io::Error),

    #[error("parse config")]
    Parse(#[source] serde_yaml::Error),

    #[error("serialize config")]
    Serialize(#[source] serde_yaml::Error),

    #[error("invalid config: {0}")]
    Invalid(String),
}

impl ConfigError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Read(err) if err.kind() == io::ErrorKind::NotFound)
    }
}

pub fn load_config<T>(path: impl AsRef<Path>) -> Result<T, ConfigError>
where
    T: DeserializeOwned,
{
    let content = fs::read_to_string(path).map_err(ConfigError::Read)?;
    let config: T = serde_yaml::from_str(&content).map_err(ConfigError::Parse)?;

    Ok(config)
}

pub fn save_config<T>(path: impl AsRef<Path>, config: &T) -> Result<(), ConfigError>
where
    T: Serialize,
{
    let yaml = serde_yaml::to_string(config).map_err(ConfigError::Serialize)?;
    fs::write(path, yaml).map_err(ConfigError::Write)?;

    Ok(())
}
