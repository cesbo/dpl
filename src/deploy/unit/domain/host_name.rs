use std::fmt;

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
};
use thiserror::Error;

#[derive(Error, Debug)]
#[error("invalid host name '{0}'")]
pub struct HostNameError(String);

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostName(String);

impl HostName {
    pub fn new(value: impl Into<String>) -> Result<Self, HostNameError> {
        let host = value.into();
        if !Self::is_valid(&host) {
            Err(HostNameError(host))
        } else {
            Ok(Self(host))
        }
    }

    pub fn is_valid(value: &str) -> bool {
        if value.is_empty() {
            return false;
        }
        if value.contains("**") {
            return false;
        }
        if value.starts_with('-') || value.starts_with('.') {
            return false;
        }
        // '*' is allowed only as the first char, and must be followed by '.'
        if let Some(rest) = value.strip_prefix('*')
            && !rest.starts_with('.')
        {
            return false;
        }
        // No '*' anywhere except possibly position 0.
        !value[1 ..].contains('*')
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for HostName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        HostName::new(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for HostName {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_name_accepts_valid() {
        assert!(HostName::new("example.com").is_ok());
        assert!(HostName::new("www.example.com").is_ok());
        assert!(HostName::new("*.example.com").is_ok());
        assert!(HostName::new("a-b.example.com").is_ok());
    }

    #[test]
    fn host_name_rejects_invalid() {
        assert!(HostName::new("").is_err());
        assert!(HostName::new("**.example.com").is_err());
        assert!(HostName::new("*example.com").is_err());
        assert!(HostName::new("*").is_err());
        assert!(HostName::new("a*.com").is_err());
        assert!(HostName::new("-bad.com").is_err());
        assert!(HostName::new(".bad.com").is_err());
    }

    #[test]
    fn host_name_serde_roundtrip() {
        let host = HostName::new("example.com").unwrap();
        let yaml = serde_yaml::to_string(&host).unwrap();
        assert_eq!(yaml.trim(), "example.com");
        let parsed: HostName = serde_yaml::from_str("example.com").unwrap();
        assert_eq!(parsed, host);
    }

    #[test]
    fn host_name_deserialize_rejects_invalid() {
        let err = serde_yaml::from_str::<HostName>("'**.x'").unwrap_err();
        assert!(err.to_string().contains("invalid host name"));
    }
}
