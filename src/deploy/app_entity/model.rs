use std::collections::BTreeMap;

use serde::{
    Deserialize,
    Serialize,
};

use crate::deploy::EntityType;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(rename = "type")]
    pub entity_type: EntityType,
    pub image: String,
    pub port: u16,
    pub build: Vec<BuildLayerConfig>,
    pub runtime: RuntimeConfig,
    pub domain: Option<String>,
    pub route: Option<String>,
    #[serde(default)]
    pub volumes: Vec<VolumeConfig>,
    #[serde(default)]
    pub exports: Vec<ExportConfig>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BuildLayerConfig {
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub script: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub init: Option<String>,
    pub cmd: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VolumeConfig {
    pub source: String,
    pub path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExportConfig {
    pub path: String,
    pub url: String,
}
