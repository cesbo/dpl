use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuthKey {
    pub name: String,
    pub token: String,
    pub apps: Vec<String>,
    #[serde(default)]
    pub disabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub keys: Vec<AuthKey>,
}

impl AuthConfig {
    pub fn validate(&self) -> Result<(), String> {
        let mut ids = std::collections::BTreeSet::new();

        for key in &self.keys {
            if !ids.insert(key.name.as_str()) {
                return Err(format!("duplicate key name: {}", key.name));
            }
        }

        Ok(())
    }
}
