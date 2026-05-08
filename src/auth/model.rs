use serde::{
    Deserialize,
    Serialize,
};

use crate::config::ValidateConfig;

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AuthEntry {
    pub hash: String,
    pub apps: Vec<String>,
}

impl ValidateConfig for AuthEntry {}
