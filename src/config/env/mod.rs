mod name;
mod value;

use std::collections::BTreeMap;

use serde::{
    Deserialize,
    Serialize,
};

pub use self::{
    name::Name,
    value::Value,
};
use crate::{
    MainContext,
    config::UnitName,
    reference::{
        Location,
        ReferenceError,
    },
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct EnvList(BTreeMap<Name, Value>);

impl EnvList {
    pub fn resolve(
        &self,
        ctx: &MainContext,
        prefix: &str,
    ) -> Result<BTreeMap<String, String>, ReferenceError> {
        self.0
            .iter()
            .map(|(k, v)| {
                let value = v
                    .render(ctx)
                    .map_err(|err| err.at(Location::field(format!("{prefix}.{k}"))))?;
                Ok((k.to_string(), value))
            })
            .collect()
    }

    /// Names of all units referenced across every entry (duplicates possible).
    ///
    /// Purely syntactic; see [`Value::unit_refs`].
    pub fn unit_refs(&self) -> impl Iterator<Item = &UnitName> {
        self.0.values().flat_map(|value| value.unit_refs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_unquoted_integer() {
        let list: EnvList = serde_yaml::from_str("DB_PORT: 5432").unwrap();
        let resolved = list.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(resolved.get("DB_PORT").map(String::as_str), Some("5432"));
    }

    #[test]
    fn deserialize_unquoted_bool_and_float() {
        let list: EnvList = serde_yaml::from_str("DEBUG: true\nRATIO: 0.5\n").unwrap();
        let resolved = list.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(resolved.get("DEBUG").map(String::as_str), Some("true"));
        assert_eq!(resolved.get("RATIO").map(String::as_str), Some("0.5"));
    }

    #[test]
    fn deserialize_rejects_template_syntax_error() {
        let err = serde_yaml::from_str::<EnvList>("BAD: \"${secret:}\"").unwrap_err();
        assert!(err.to_string().contains("malformed reference"));
    }

    #[test]
    fn unit_refs_across_entries_skipping_secrets() {
        let list: EnvList = serde_yaml::from_str(
            "DB_URL: \"${app-db:url}\"\nCACHE: \"${secret:redis}\"\nUPSTREAM: \"${api:url}\"\n",
        )
        .unwrap();
        // BTreeMap iterates by key (CACHE, DB_URL, UPSTREAM); secret ref dropped.
        let names: Vec<&str> = list.unit_refs().map(UnitName::as_str).collect();
        assert_eq!(names, vec!["app-db", "api"]);
    }
}
