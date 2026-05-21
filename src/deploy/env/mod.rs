mod value;

use std::collections::BTreeMap;

use serde::{
    Deserialize,
    Serialize,
};

pub use self::value::Value;
use crate::{
    MainContext,
    error::{
        Location,
        RefError,
    },
};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct EnvList(BTreeMap<String, Value>);

impl EnvList {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn resolve(
        &self,
        ctx: &MainContext,
        prefix: &str,
    ) -> Result<BTreeMap<String, String>, RefError> {
        self.0
            .iter()
            .map(|(k, v)| {
                let value = v
                    .render(ctx)
                    .map_err(|err| err.at(Location::field(format!("{prefix}.{k}"))))?;
                Ok((k.clone(), value))
            })
            .collect()
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
}
