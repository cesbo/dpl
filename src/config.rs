use std::{
    fmt,
    fs,
    io,
    path::{
        Path,
        PathBuf,
    },
};

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
    de::DeserializeOwned,
};
use thiserror::Error;

use crate::MainContext;

#[derive(Error, Debug)]
#[error("invalid resource name '{0}'")]
pub struct ResourceNameError(String);

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceName(String);

impl ResourceName {
    pub fn new(value: impl Into<String>) -> Result<Self, ResourceNameError> {
        let name = value.into();
        if !Self::is_valid(&name) {
            Err(ResourceNameError(name))
        } else {
            Ok(Self(name))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        if name.starts_with('-') || name.ends_with('-') {
            return false;
        }

        if name.contains("--") {
            return false;
        }

        name.as_bytes()
            .iter()
            .all(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    }

    pub fn unit_dir(&self, ctx: &MainContext) -> PathBuf {
        ctx.base().join(self.as_str())
    }
}

impl fmt::Display for ResourceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ResourceName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        ResourceName::new(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for ResourceName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_name_accepts_valid() {
        assert!(ResourceName::new("foo").is_ok());
        assert!(ResourceName::new("a-b-c").is_ok());
        assert!(ResourceName::new("app-1").is_ok());
        assert!(ResourceName::new("0").is_ok());
    }

    #[test]
    fn resource_name_rejects_invalid() {
        assert!(ResourceName::new("").is_err());
        assert!(ResourceName::new("-foo").is_err());
        assert!(ResourceName::new("foo-").is_err());
        assert!(ResourceName::new("foo--bar").is_err());
        assert!(ResourceName::new("Foo").is_err());
        assert!(ResourceName::new("foo_bar").is_err());
        assert!(ResourceName::new("foo.bar").is_err());
        assert!(ResourceName::new("foo/bar").is_err());
        assert!(ResourceName::new(" foo").is_err());
    }

    #[test]
    fn resource_name_serde_roundtrip() {
        let name = ResourceName::new("pg-main").unwrap();
        let yaml = serde_yaml::to_string(&name).unwrap();
        assert_eq!(yaml.trim(), "pg-main");
        let parsed: ResourceName = serde_yaml::from_str("pg-main").unwrap();
        assert_eq!(parsed, name);
    }

    #[test]
    fn resource_name_deserialize_rejects_invalid() {
        let err = serde_yaml::from_str::<ResourceName>("Bad/Name").unwrap_err();
        assert!(err.to_string().contains("invalid resource name"));
    }
}
