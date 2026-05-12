use std::{
    any::Any,
    fs,
    io,
    path::Path,
};

use serde::{
    Serialize,
    de::DeserializeOwned,
};
use thiserror::Error;

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

pub fn load_config<T>(path: impl AsRef<Path>) -> Result<T, ConfigError>
where
    T: DeserializeOwned + ValidateConfig,
{
    let content = match fs::read_to_string(path) {
        Ok(v) => v,
        Err(err) => {
            if err.kind() == io::ErrorKind::NotFound
                && let Some(v) = T::default_config()
            {
                return Ok(v);
            }

            return Err(ConfigError::Read(err));
        }
    };

    let config: T = serde_yaml::from_str(&content).map_err(ConfigError::Parse)?;
    config.validate_config().map_err(ConfigError::Invalid)?;

    Ok(config)
}

pub fn save_config<T>(path: impl AsRef<Path>, config: &T) -> Result<(), ConfigError>
where
    T: Serialize,
{
    let yaml = serde_yaml::to_string(config).map_err(ConfigError::Serialize)?;

    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(ConfigError::Write)?;
    }

    fs::write(path, yaml).map_err(ConfigError::Write)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, Default)]
    #[serde(default)]
    struct TestConfig {
        value: i32,
    }

    impl ValidateConfig for TestConfig {
        fn default_config() -> Option<Self> {
            Some(Self { value: 42 })
        }

        fn validate_config(&self) -> Result<(), String> {
            if self.value < 0 {
                Err(format!("value must be non-negative, got {}", self.value))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn load_config_uses_default_when_file_missing() {
        let missing = Path::new("/nonexistent.yaml");
        let config: TestConfig = load_config(missing).expect("should fall back to default");
        assert_eq!(config.value, 42);
    }

    #[test]
    fn load_config_reports_invalid_when_validation_fails() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"value: -1\n").unwrap();

        if let Err(ConfigError::Invalid(info)) = load_config::<TestConfig>(file.path()) {
            assert!(info.contains("non-negative"), "unexpected info: {info}");
        } else {
            panic!("unexpected result");
        }
    }
}
