use std::fmt;

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    App,
    Domain,
    Static,
    Database,
}

impl fmt::Display for EntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::App => "app",
            Self::Domain => "domain",
            Self::Static => "static",
            Self::Database => "database",
        };

        f.write_str(value)
    }
}
