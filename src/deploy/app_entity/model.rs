use std::collections::BTreeSet;

use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    config::ValidateConfig,
    deploy::EnvList,
    validate,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
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
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildLayerConfig {
    /// Description
    pub description: Option<String>,
    /// Files to include in the build layer
    #[serde(default)]
    pub files: Vec<String>,
    /// Environment variables for the build layer
    #[serde(default)]
    pub env: EnvList,
    /// Shell script to execute for the build layer
    pub script: Option<String>,
}

/// Configuration for the runtime environment of the application
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Environment variables
    #[serde(default)]
    pub env: EnvList,
    /// Shell script to initialize the runtime environment
    pub init: Option<String>,
    /// Command to run the application
    pub cmd: String,
}

/// Creates a bind mount
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VolumeConfig {
    /// Description
    pub description: Option<String>,
    /// Source is a podman volume name or full path to the host directory
    pub source: String,
    /// Path inside the container where the volume will be mounted
    pub path: String,
}

/// Exports static files from the container to the host
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExportConfig {
    /// Description
    pub description: Option<String>,
    /// Path inside the container where static files located
    pub source: String,
    /// URL path where the exported files will be accessible
    pub path: String,
}

/// Timers to start scripts periodically in the container
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TimerConfig {
    /// Name
    pub name: String,
    /// Description
    pub description: Option<String>,
    /// Schedule in systemd OnCalendar format
    pub schedule: String,
    /// Script to run
    pub script: String,
}

impl ValidateConfig for TimerConfig {
    fn validate_config(&self) -> Result<(), String> {
        if !validate::resource_name(&self.name) {
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

impl ValidateConfig for AppConfig {
    fn validate_config(&self) -> Result<(), String> {
        let mut names = BTreeSet::new();

        self.runtime.env.validate_config()?;

        for timer in &self.timers {
            timer.validate_config()?;

            if !names.insert(timer.name.as_str()) {
                return Err(format!("duplicate timer name: {}", timer.name));
            }
        }

        for export in &self.exports {
            if !validate::url_path(&export.path) {
                return Err(format!("invalid export path: '{}'", export.path));
            }
        }

        Ok(())
    }
}
