use std::fmt;

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
};
use thiserror::Error;

#[derive(Error, Debug)]
#[error("invalid unit name '{0}'")]
pub struct UnitNameError(String);

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitName(String);

impl UnitName {
    pub fn new(value: impl Into<String>) -> Result<Self, UnitNameError> {
        let name = value.into();
        if !Self::is_valid(&name) {
            Err(UnitNameError(name))
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

    /// Scoped name of a dpl-managed unit: `dpl--<name>`.
    pub fn scoped_unit_name(&self) -> String {
        format!("dpl--{name}", name = self.as_str())
    }
}

impl std::str::FromStr for UnitName {
    type Err = UnitNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl fmt::Display for UnitName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for UnitName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for UnitName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        UnitName::new(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for UnitName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_name_accepts_valid() {
        assert!(UnitName::new("foo").is_ok());
        assert!(UnitName::new("a-b-c").is_ok());
        assert!(UnitName::new("app-1").is_ok());
        assert!(UnitName::new("0").is_ok());
    }

    #[test]
    fn unit_name_rejects_invalid() {
        assert!(UnitName::new("").is_err());
        assert!(UnitName::new("-foo").is_err());
        assert!(UnitName::new("foo-").is_err());
        assert!(UnitName::new("foo--bar").is_err());
        assert!(UnitName::new("Foo").is_err());
        assert!(UnitName::new("foo_bar").is_err());
        assert!(UnitName::new("foo.bar").is_err());
        assert!(UnitName::new("foo/bar").is_err());
        assert!(UnitName::new(" foo").is_err());
    }

    #[test]
    fn unit_name_serde_roundtrip() {
        let name = UnitName::new("pg-main").unwrap();
        let yaml = serde_yaml::to_string(&name).unwrap();
        assert_eq!(yaml.trim(), "pg-main");
        let parsed: UnitName = serde_yaml::from_str("pg-main").unwrap();
        assert_eq!(parsed, name);
    }

    #[test]
    fn unit_name_deserialize_rejects_invalid() {
        let err = serde_yaml::from_str::<UnitName>("Bad/Name").unwrap_err();
        assert!(err.to_string().contains("invalid unit name"));
    }
}
