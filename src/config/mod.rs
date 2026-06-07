mod env;
mod error;
mod schedule;
mod secret_name;
mod string_from_scalar;
mod unit_name;

pub use self::{
    env::{
        EnvList,
        Value,
    },
    error::ConfigError,
    schedule::Schedule,
    secret_name::SecretName,
    string_from_scalar::deserialize_optional_string_from_scalar,
    unit_name::UnitName,
};

pub fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    v == &T::default()
}
