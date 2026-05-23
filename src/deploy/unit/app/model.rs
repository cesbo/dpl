use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    MainContext,
    config::ResourceName,
    deploy::{
        EnvList,
        UnitConfig,
    },
    error::{
        Location,
        RefError,
    },
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub image: String,
    pub port: u16,
    pub builds: Vec<BuildConfig>,
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub volumes: Vec<VolumeConfig>,
    #[serde(default)]
    pub exports: Vec<ExportConfig>,
    #[serde(default)]
    pub timers: Vec<TimerConfig>,
    #[serde(default)]
    pub databases: Vec<ResourceName>,
}

/// Configuration for a build layer of the application
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildConfig {
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

impl AppConfig {
    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        self.runtime.env.resolve(ctx, "runtime.env")?;

        for (index, layer) in self.builds.iter().enumerate() {
            layer.env.resolve(ctx, &format!("builds[{index}].env"))?;
        }

        for (index, db) in self.databases.iter().enumerate() {
            let inner: RefError = match UnitConfig::load(ctx, db) {
                Ok(UnitConfig::Db(config)) => match config.validate_references(ctx) {
                    Ok(()) => continue,
                    Err(err) => err.at(Location::unit(db.as_str())),
                },
                Ok(_) => RefError::wrong_unit_type(db.to_string(), "db"),
                Err(err) => err.into(),
            };
            return Err(inner.at(Location::field(format!("databases[{index}]"))));
        }

        Ok(())
    }

    pub fn resolve_export(
        &self,
        _ctx: &MainContext,
        unit_name: &ResourceName,
        key: &str,
    ) -> Result<String, RefError> {
        match key {
            // The app is reachable by other units over the private `dpl`
            // container network at its container name and listening port.
            "url" => Ok(format!("http://dpl-{unit_name}:{port}", port = self.port)),
            _ => Err(RefError::unknown_export(key)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> AppConfig {
        AppConfig {
            image: "alpine".into(),
            port: 8080,
            builds: Vec::new(),
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
    fn app_resolve_export_url() {
        let ctx = MainContext::default();
        let config = sample_config();
        assert_eq!(
            config
                .resolve_export(&ctx, &ResourceName::new("web").unwrap(), "url")
                .unwrap(),
            "http://dpl-web:8080"
        );
    }

    #[test]
    fn app_resolve_export_unknown_key() {
        let ctx = MainContext::default();
        let config = sample_config();
        assert!(
            config
                .resolve_export(&ctx, &ResourceName::new("web").unwrap(), "nope")
                .is_err()
        );
    }
}
