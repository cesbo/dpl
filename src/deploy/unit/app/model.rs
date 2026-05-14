use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    MainContext,
    config::ValidateConfig,
    deploy::{
        EnvList,
        UnitConfig,
    },
    validate,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
    #[serde(default)]
    pub databases: Vec<String>,
}

/// Configuration for a build layer of the application
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
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

impl AppConfig {
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), String> {
        if let Err(err) = self.runtime.env.validate_references(ctx) {
            return Err(format!("runtime env: {err}"));
        }

        for layer in &self.build {
            if let Err(err) = layer.env.validate_references(ctx) {
                return Err(format!("build env: {err}"));
            }
        }

        for db in &self.databases {
            match UnitConfig::load(ctx, db) {
                Ok(UnitConfig::Db(_)) => {}
                Ok(_) => {
                    return Err(format!("databases: unit '{db}' is not a database"));
                }
                Err(err) => {
                    return Err(format!("databases: {err}"));
                }
            }
        }

        Ok(())
    }

    pub fn has_export(key: &str) -> bool {
        matches!(key, "url")
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        unit_name: &str,
        key: &str,
    ) -> Result<String, String> {
        match key {
            "url" => {
                let unit_dir = ctx.base().join(unit_name);
                let port = super::port::read_port(&unit_dir)
                    .map_err(|err| format!("resolve app port: {err}"))?
                    .ok_or_else(|| format!("app '{unit_name}' is not deployed yet"))?;
                Ok(format!("http://127.0.0.1:{port}"))
            }
            _ => Err(format!("unknown export '{key}'")),
        }
    }
}

impl ValidateConfig for AppConfig {
    fn validate_config(&self) -> Result<(), String> {
        self.runtime.env.validate_config()?;
        for layer in &self.build {
            layer.env.validate_config()?;
        }

        for timer in &self.timers {
            timer.validate_config()?;
        }

        for export in &self.exports {
            if !validate::url_path(&export.path) {
                return Err(format!("invalid export path: '{}'", export.path));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn sample_config() -> AppConfig {
        AppConfig {
            image: "alpine".into(),
            port: 8080,
            build: Vec::new(),
            runtime: RuntimeConfig {
                env: EnvList::default(),
                init: None,
                cmd: "./run".into(),
            },
            volumes: Vec::new(),
            exports: Vec::new(),
            timers: Vec::new(),
            databases: Vec::new(),
        }
    }

    #[test]
    fn app_has_export() {
        assert!(AppConfig::has_export("url"));
        assert!(!AppConfig::has_export("unknown"));
    }

    #[test]
    fn app_resolve_export_url() {
        let base = TempDir::new().unwrap();
        let unit_dir = base.path().join("web");
        fs::create_dir_all(&unit_dir).unwrap();
        fs::write(unit_dir.join("port.txt"), "12345").unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config = sample_config();
        assert_eq!(
            config.resolve_export(&ctx, "web", "url").unwrap(),
            "http://127.0.0.1:12345"
        );
    }

    #[test]
    fn app_resolve_export_port_missing() {
        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config = sample_config();
        let err = config.resolve_export(&ctx, "web", "port").unwrap_err();
        assert!(
            err.contains("is not deployed yet"),
            "unexpected error: {err}"
        );
    }
}
