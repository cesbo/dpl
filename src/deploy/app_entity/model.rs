use std::collections::{
    BTreeMap,
    BTreeSet,
};

use serde::{
    Deserialize,
    Serialize,
};

use crate::deploy::{
    EntityType,
    entity::validate_name,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    #[serde(rename = "type")]
    pub entity_type: EntityType,
    pub image: String,
    pub port: u16,
    pub build: Vec<BuildLayerConfig>,
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub volumes: Vec<VolumeConfig>,
    #[serde(default)]
    pub exports: Vec<ExportConfig>,
    #[serde(default)]
    pub timers: Vec<TimerConfig>,
}

/// Configuration for a build layer of the application
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BuildLayerConfig {
    /// Files to include in the build layer
    #[serde(default)]
    pub files: Vec<String>,
    /// Environment variables for the build layer
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Shell script to execute for the build layer
    pub script: Option<String>,
}

/// Configuration for the runtime environment of the application
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Environment variables
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Shell script to initialize the runtime environment
    pub init: Option<String>,
    /// Command to run the application
    pub cmd: String,
}

/// Creates a bind mount
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct VolumeConfig {
    /// Source is a podman volume name or full path to the host directory
    pub source: String,
    /// Path inside the container where the volume will be mounted
    pub path: String,
}

/// Exports static files from the container to the host
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExportConfig {
    /// Path inside the container where static files located
    pub source: String,
    /// URL where the exported files will be accessible
    pub url: String,
}

/// Timers to start scripts periodically in the container
#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimerConfig {
    /// Name of the timer
    pub name: String,
    /// Schedule in systemd OnCalendar format
    pub schedule: String,
    /// Script to run
    pub script: String,
}

impl TimerConfig {
    fn validate(&self) -> Result<(), String> {
        if !validate_name(&self.name) {
            return Err(format!("invalid timer name: '{}'", self.name));
        }

        if self.schedule.is_empty() {
            return Err(format!("timer '{}' has empty schedule", self.name));
        }

        if self.script.is_empty() {
            return Err(format!("timer '{}' has empty script", self.name));
        }

        Ok(())
    }
}

impl AppConfig {
    pub fn validate(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();

        for timer in &self.timers {
            timer.validate()?;

            if !names.insert(timer.name.as_str()) {
                return Err(format!("duplicate timer name: {}", timer.name));
            }
        }

        Ok(())
    }
}
