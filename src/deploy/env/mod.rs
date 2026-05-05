mod value;

use std::{
    collections::{
        BTreeMap,
        BTreeSet,
    },
    path::Path,
};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    config::ValidateConfig,
    secret::SecretError,
    validate,
};

use value::{
    Ns,
    Value,
};

#[derive(Debug, Error)]
pub enum EnvError {
    #[error("secret")]
    Secret(#[from] SecretError),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
pub struct EnvList(BTreeMap<String, Value>);

impl EnvList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve(&self) -> Result<BTreeMap<String, String>, EnvError> {
        let refs = self.collect_references();

        let mut resolved: BTreeMap<Ns, BTreeMap<String, String>> = BTreeMap::new();
        for (ns, names) in refs {
            let entry = match ns {
                Ns::Secret => resolve_secrets(&names)?,
            };
            resolved.insert(ns, entry);
        }

        Ok(self
            .0
            .iter()
            .map(|(k, v)| (k.clone(), v.render(&resolved)))
            .collect())
    }

    pub fn validate_references(&self, base: &Path) -> Result<(), String> {
        for (ns, names) in self.collect_references() {
            match ns {
                Ns::Secret => {
                    for name in names {
                        if !crate::secret::secret_exists(base, name) {
                            return Err(format!("secret '{name}' does not exist"));
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn collect_references(&self) -> BTreeMap<Ns, BTreeSet<&str>> {
        let mut refs: BTreeMap<Ns, BTreeSet<&str>> = BTreeMap::new();
        for value in self.0.values() {
            for (ns, name) in value.references() {
                refs.entry(ns.clone()).or_default().insert(name);
            }
        }
        refs
    }
}

#[cfg(test)]
impl EnvList {
    pub fn insert_literal(&mut self, key: String, value: String) {
        self.0
            .insert(key, Value::parse(&value).expect("literal must not contain '$'"));
    }
}

fn resolve_secrets(names: &BTreeSet<&str>) -> Result<BTreeMap<String, String>, EnvError> {
    let master_key = crate::secret::MasterKey::load(crate::config().base.as_path())?;
    names
        .iter()
        .map(|name| Ok(((*name).to_owned(), master_key.decrypt_from_file(name)?)))
        .collect()
}

impl ValidateConfig for EnvList {
    fn validate_config(&self) -> Result<(), String> {
        for key in self.0.keys() {
            if !validate::env_name(key) {
                return Err(format!("invalid env name: '{key}'"));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn list_from_yaml(yaml: &str) -> EnvList {
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn deserialize_literal_and_template() {
        let list = list_from_yaml(
            r#"
PORT: "8080"
DATABASE_URL: "postgres://app:${secret:db}@host/app"
TOKEN: "${secret:api-token}"
"#,
        );
        assert_eq!(list.0.len(), 3);
    }

    #[test]
    fn deserialize_rejects_template_syntax_error() {
        let err = serde_yaml::from_str::<EnvList>("BAD: \"${secret:}\"").unwrap_err();
        assert!(err.to_string().contains("malformed reference"));
    }

    #[test]
    fn validate_config_rejects_invalid_env_name() {
        let mut list = EnvList::new();
        list.insert_literal("BAD-NAME".into(), "x".into());
        assert!(list.validate_config().is_err());
    }

    #[test]
    fn validate_references_reports_missing_secret() {
        let list = list_from_yaml(r#"X: "${secret:nope}""#);
        let err = list.validate_references(&PathBuf::from("/nonexistent")).unwrap_err();
        assert!(err.contains("nope"));
    }
}
