use std::str::FromStr;

use croner::{
    Cron,
    errors::CronError,
};
use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    Serializer,
};

/// A timer's cron schedule. Thin newtype over `croner::Cron`.
#[derive(Clone, Debug, PartialEq)]
pub struct Schedule(Cron);

impl Schedule {
    pub fn as_cron(&self) -> &Cron {
        &self.0
    }
}

impl FromStr for Schedule {
    type Err = CronError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Cron::from_str(s).map(Schedule)
    }
}

impl<'de> Deserialize<'de> for Schedule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Cron::from_str(&value).map(Schedule).map_err(|err| {
            serde::de::Error::custom(format!("invalid timer schedule {value:?}: {err}"))
        })
    }
}

impl Serialize for Schedule {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_standard_five_field() {
        let s: Schedule = serde_yaml::from_str("\"0 2 * * *\"").unwrap();
        assert_eq!(s.as_cron().pattern.to_string(), "0 2 * * *");
    }

    #[test]
    fn rejects_named_keyword_with_value_and_label() {
        let err = serde_yaml::from_str::<Schedule>("minutely").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid timer schedule"), "got: {msg}");
        assert!(msg.contains("\"minutely\""), "got: {msg}");
    }

    #[test]
    fn serialize_roundtrip() {
        let s: Schedule = serde_yaml::from_str("\"*/5 * * * *\"").unwrap();
        let yaml = serde_yaml::to_string(&s).unwrap();
        let back: Schedule = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(s, back);
    }
}
