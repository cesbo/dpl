use std::{
    fs,
    io,
    path::{
        Path,
        PathBuf,
    },
};

use serde::de::DeserializeOwned;
use thiserror::Error;

pub trait ValidateConfig {
    /// Config required by the default
    fn default_config() -> Option<Self>
    where
        Self: Sized,
    {
        None
    }

    fn validate_config(&self) -> Result<(), String> {
        Ok(())
    }
}

pub fn load_config<T>(path: &Path) -> Result<T, ConfigError>
where
    T: DeserializeOwned + ValidateConfig,
{
    let content = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(source) => {
            if source.kind() == io::ErrorKind::NotFound
                && let Some(v) = T::default_config()
            {
                return Ok(v);
            }

            return Err(ConfigError::Read {
                path: path.into(),
                source,
            });
        }
    };

    let config: T = serde_yaml::from_str(&content).map_err(|source| ConfigError::Parse {
        path: path.into(),
        source,
    })?;

    config
        .validate_config()
        .map_err(|info| ConfigError::Invalid {
            path: path.into(),
            info,
        })?;

    Ok(config)
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("read config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_yaml::Error,
    },

    #[error("invalid configuration {path}: {info}")]
    Invalid { path: PathBuf, info: String },
}

impl ConfigError {
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::Read { source, .. } if source.kind() == io::ErrorKind::NotFound)
    }
}
