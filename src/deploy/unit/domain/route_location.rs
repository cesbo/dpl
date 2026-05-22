use std::fmt;

use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
};
use thiserror::Error;

#[derive(Error, Debug)]
#[error("invalid route location '{0}'")]
pub struct RouteLocationError(String);

#[repr(transparent)]
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RouteLocation(String);

impl RouteLocation {
    pub fn new(value: impl Into<String>) -> Result<Self, RouteLocationError> {
        let location = value.into();
        if !Self::is_valid(&location) {
            Err(RouteLocationError(location))
        } else {
            Ok(Self(location))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_valid(value: &str) -> bool {
        !value.is_empty()
            && value.starts_with('/')
            && !value.chars().any(char::is_whitespace)
    }
}

impl fmt::Display for RouteLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for RouteLocation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        RouteLocation::new(value).map_err(serde::de::Error::custom)
    }
}

impl Serialize for RouteLocation {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_location_accepts_valid() {
        assert!(RouteLocation::new("/").is_ok());
        assert!(RouteLocation::new("/api").is_ok());
        assert!(RouteLocation::new("/billing/static").is_ok());
    }

    #[test]
    fn route_location_rejects_invalid() {
        assert!(RouteLocation::new("").is_err());
        assert!(RouteLocation::new("api").is_err());
        assert!(RouteLocation::new("/a b").is_err());
    }

    #[test]
    fn route_location_serde_roundtrip() {
        let location = RouteLocation::new("/api").unwrap();
        let yaml = serde_yaml::to_string(&location).unwrap();
        assert_eq!(yaml.trim(), "/api");
        let parsed: RouteLocation = serde_yaml::from_str("/api").unwrap();
        assert_eq!(parsed, location);
    }

    #[test]
    fn route_location_deserialize_rejects_invalid() {
        let err = serde_yaml::from_str::<RouteLocation>("api").unwrap_err();
        assert!(err.to_string().contains("invalid route location"));
    }
}
