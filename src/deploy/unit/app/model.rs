use std::collections::BTreeSet;

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
        state::DeployState,
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
    /// Units referenced through `${unit:key}` tokens across `runtime.env` and
    /// every build layer's `env`, deduplicated and sorted.
    pub fn unit_deps(&self) -> BTreeSet<ResourceName> {
        let mut deps: BTreeSet<ResourceName> = self.runtime.env.unit_refs().cloned().collect();
        for layer in &self.builds {
            deps.extend(layer.env.unit_refs().cloned());
        }
        deps
    }

    pub fn database_deps(&self, ctx: &MainContext) -> Result<Vec<ResourceName>, RefError> {
        let mut dbs = Vec::new();
        for dep in self.unit_deps() {
            match UnitConfig::load(ctx, &dep) {
                Ok(UnitConfig::Db(_)) => dbs.push(dep),
                Ok(_) => continue,
                Err(err) => return Err(RefError::from(err).at(Location::unit(dep.as_str()))),
            }
        }
        Ok(dbs)
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        self.runtime.env.resolve(ctx, "runtime.env")?;

        for (index, layer) in self.builds.iter().enumerate() {
            layer.env.resolve(ctx, &format!("builds[{index}].env"))?;
        }

        for dep in self.unit_deps() {
            match UnitConfig::load(ctx, &dep) {
                Ok(UnitConfig::Db(config)) => config
                    .validate_references(ctx)
                    .map_err(|err| err.at(Location::unit(dep.as_str())))?,
                Ok(_) => continue,
                Err(err) => return Err(RefError::from(err).at(Location::unit(dep.as_str()))),
            }
        }

        Ok(())
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        unit_name: &ResourceName,
        key: &str,
    ) -> Result<String, RefError> {
        match key {
            // The app is reachable by other units over the private `dpl`
            // container network at its container name and listening port.
            "url" => Ok(format!("http://dpl-{unit_name}:{port}", port = self.port)),
            // Static export directory inside the nginx www volume. The nginx
            // container prepends its own mount base to this in-volume path.
            "export" => {
                let unit_dir = unit_name.unit_dir(ctx);
                let version = DeployState::get_active_version(&unit_dir)
                    .map_err(|_| RefError::not_deployed(unit_name.as_str()))?;
                Ok(format!("/{unit_name}_{version}"))
            }
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
        }
    }

    #[test]
    fn app_unit_deps_from_env_and_builds() {
        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nport: 8080\nruntime:\n  cmd: ./run\n  env:\n    DB: \"${app-db:url}\"\n    SECRET: \"${secret:k}\"\nbuilds:\n  - env:\n      API: \"${api:url}\"\n",
        )
        .unwrap();
        // BTreeSet → sorted, deduped, secret ref dropped.
        let deps = config.unit_deps();
        let names: Vec<&str> = deps.iter().map(ResourceName::as_str).collect();
        assert_eq!(names, vec!["api", "app-db"]);
    }

    #[test]
    fn app_unit_deps_empty_without_refs() {
        assert!(sample_config().unit_deps().is_empty());
    }

    #[test]
    fn app_database_deps_keeps_db_skips_other_kinds() {
        use std::fs;

        use tempfile::TempDir;

        // app `foo` references a `db` unit and another `app` unit. Only the db
        // is a startup dependency; the app reference is dropped.
        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nport: 8080\nruntime:\n  cmd: ./run\n  env:\n    DB: \"${db-x:url}\"\n    UPSTREAM: \"${other-app:url}\"\nbuilds: []\n",
        )
        .unwrap();

        let base = TempDir::new().unwrap();
        let db_dir = base.path().join("db-x");
        fs::create_dir_all(&db_dir).unwrap();
        fs::write(
            db_dir.join("config.yaml"),
            "type: db\nserver: pg-main\nuser: app1\nsecret: db-x-pass\n",
        )
        .unwrap();
        let app_dir = base.path().join("other-app");
        fs::create_dir_all(&app_dir).unwrap();
        fs::write(
            app_dir.join("config.yaml"),
            "type: app\nimage: alpine\nport: 9090\nbuilds: []\nruntime:\n  cmd: ./run\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let deps = config.database_deps(&ctx).unwrap();
        let names: Vec<&str> = deps.iter().map(ResourceName::as_str).collect();
        assert_eq!(names, vec!["db-x"]);
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

    #[test]
    fn app_resolve_export_dir() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let unit_dir = base.path().join("web");
        fs::create_dir_all(&unit_dir).unwrap();
        fs::write(
            unit_dir.join("state.yaml"),
            "active_version: 3\nlatest_build:\n  version: 3\n  status: ready\n",
        )
        .unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        assert_eq!(
            sample_config()
                .resolve_export(&ctx, &ResourceName::new("web").unwrap(), "export")
                .unwrap(),
            "/web_3"
        );
    }

    #[test]
    fn app_resolve_export_not_deployed() {
        use tempfile::TempDir;

        use crate::error::RefErrorKind;

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        // No state.yaml on disk → no active deployment to export from.
        let err = sample_config()
            .resolve_export(&ctx, &ResourceName::new("web").unwrap(), "export")
            .unwrap_err();
        assert!(matches!(err.kind, RefErrorKind::NotDeployed { name } if name == "web"));
    }
}
