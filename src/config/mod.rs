mod env;
mod error;
mod secret_name;
mod string_from_scalar;
mod unit_name;

use std::str::FromStr;

use serde::Deserialize;

pub use self::{
    env::{
        EnvList,
        Value,
    },
    error::ConfigError,
    secret_name::SecretName,
    string_from_scalar::deserialize_optional_string_from_scalar,
    unit_name::UnitName,
};

pub fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    v == &T::default()
}

pub fn deserialize_cron<'de, D>(deserializer: D) -> Result<croner::Cron, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    let value = String::deserialize(deserializer)?;

    croner::Cron::from_str(&value)
        .map_err(|err| D::Error::custom(format!("invalid cron schedule `{value}`: {err}")))
}
