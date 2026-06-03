use std::fmt;

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
};
use thiserror::Error;

#[derive(Error, Debug)]
#[error("invalid environment variable name '{0}'")]
pub struct NameError(String);

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(String);

impl Name {
    pub fn new(value: impl Into<String>) -> Result<Self, NameError> {
        let name = value.into();
        if !Self::is_valid(&name) {
            Err(NameError(name))
        } else {
            Ok(Self(name))
        }
    }

    pub fn is_valid(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        if name.starts_with(|c: char| c.is_ascii_digit()) {
            return false;
        }

        name.as_bytes()
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Name {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Name::new(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for Name {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_accepts_valid() {
        assert!(Name::new("FOO").is_ok());
        assert!(Name::new("foo_bar").is_ok());
        assert!(Name::new("_private").is_ok());
        assert!(Name::new("X1").is_ok());
        assert!(Name::new("MIXED_Case_42").is_ok());
    }

    #[test]
    fn name_rejects_invalid() {
        assert!(Name::new("").is_err());
        assert!(Name::new("1FOO").is_err());
        assert!(Name::new("FOO-BAR").is_err());
        assert!(Name::new("FOO BAR").is_err());
        assert!(Name::new("FOO.BAR").is_err());
        assert!(Name::new("ÜMLAUT").is_err());
    }

    #[test]
    fn name_serde_roundtrip() {
        let name = Name::new("NODE_ENV").unwrap();
        let yaml = serde_yaml::to_string(&name).unwrap();
        assert_eq!(yaml.trim(), "NODE_ENV");
        let parsed: Name = serde_yaml::from_str("NODE_ENV").unwrap();
        assert_eq!(parsed, name);
    }

    #[test]
    fn name_deserialize_rejects_invalid() {
        let err = serde_yaml::from_str::<Name>("1BAD").unwrap_err();
        assert!(
            err.to_string()
                .contains("invalid environment variable name")
        );
    }
}
