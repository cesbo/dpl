use std::collections::BTreeMap;

use serde::{
    Deserialize,
    Deserializer,
};
use thiserror::Error;

use crate::{
    config::ValidateConfig,
    secret::SecretError,
    validate,
};

#[derive(Debug, Error)]
pub enum EnvError {
    #[error("secret: {0}")]
    Secret(#[from] SecretError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EnvValue {
    Plain(String),
    Secret(String),
}

impl<'de> Deserialize<'de> for EnvValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        match value {
            serde_yaml::Value::String(s) => Ok(EnvValue::Plain(s)),
            serde_yaml::Value::Tagged(tagged) => {
                let tag = tagged.tag.to_string();
                let inner = match tagged.value {
                    serde_yaml::Value::String(s) => s,
                    other => {
                        return Err(serde::de::Error::custom(format!(
                            "tag {} expects string, got {:?}",
                            tag, other
                        )));
                    }
                };
                match tag.as_str() {
                    "!secret" => Ok(EnvValue::Secret(inner)),
                    other => Err(serde::de::Error::custom(format!("unknown tag: {}", other))),
                }
            }
            _ => Err(serde::de::Error::custom("expected string or tagged scalar")),
        }
    }
}

impl ValidateConfig for EnvValue {
    fn validate_config(&self) -> Result<(), String> {
        match self {
            EnvValue::Plain(_) => Ok(()),
            EnvValue::Secret(name) => {
                if validate::secret_name(name) {
                    Ok(())
                } else {
                    Err(format!("invalid secret name: '{name}'"))
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct EnvList(BTreeMap<String, EnvValue>);

impl EnvList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert_plain(&mut self, key: String, value: String) {
        self.0.insert(key, EnvValue::Plain(value));
    }

    pub fn insert_secret(&mut self, key: String, value: String) {
        self.0.insert(key, EnvValue::Secret(value));
    }

    pub fn resolve(&self) -> Result<BTreeMap<String, String>, EnvError> {
        let mut result = BTreeMap::new();
        let mut secrets: BTreeMap<String, String> = BTreeMap::new();

        for (key, value) in &self.0 {
            let key = key.to_owned();
            match value {
                EnvValue::Plain(value) => {
                    result.insert(key, value.into());
                }
                EnvValue::Secret(name) => {
                    secrets.insert(key, name.into());
                }
            }
        }

        if secrets.is_empty() {
            return Ok(result);
        }

        let base = crate::config().base.as_path();
        let master_key = crate::secret::MasterKey::load(base)?;
        for (key, name) in secrets {
            let value = master_key.decrypt_from_file(&name)?;
            result.insert(key, value);
        }

        Ok(result)
    }

    pub fn validate_references(&self) -> Result<(), EnvError> {
        let mut count = 0;

        let base = crate::config().base.as_path();

        for value in self.0.values() {
            if let EnvValue::Secret(name) = value {
                if !crate::secret::secret_exists(base, name) {
                    return Err((SecretError::SecretNotFound { name: name.into() }).into());
                }
                count += 1;
            }
        }

        if count > 0 && !crate::secret::master_key_exists(base) {
            return Err(SecretError::KeyNotFound.into());
        }

        Ok(())
    }
}

impl ValidateConfig for EnvList {
    fn validate_config(&self) -> Result<(), String> {
        for (key, value) in &self.0 {
            if !validate::env_name(key) {
                return Err(format!("invalid env key: '{key}'"));
            }

            value.validate_config()?;
        }

        Ok(())
    }
}
