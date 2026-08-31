use std::collections::BTreeSet;

use croner::Cron;
use serde::{
    Deserialize,
    Deserializer,
    Serialize,
    de,
};

use crate::{
    MainContext,
    config::{
        EnvList,
        UnitName,
    },
    deploy::UnitConfig,
    podman::NGINX_WWW_MOUNT,
    reference::{
        Location,
        ReferenceError,
    },
    state::DeployState,
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub image: String,
    pub builds: Vec<BuildConfig>,
    #[serde(default)]
    pub runtime: Option<RuntimeConfig>,
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
    /// Port the application listens on. Omitted for a service that listens on
    /// nothing (a worker): no `EXPOSE`, no port readiness check, and no
    /// `${<unit>:url}`/`${<unit>:socket}` export.
    #[serde(default)]
    pub port: Option<u16>,
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
    #[serde(deserialize_with = "deserialize_absolute_container_path")]
    pub path: String,
}

/// Exports static files from the container to the host
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExportConfig {
    /// Description
    pub description: Option<String>,
    /// Absolute path inside the container where static files are located
    #[serde(deserialize_with = "deserialize_absolute_container_path")]
    pub source: String,
    /// URL path where the exported files will be accessible
    #[serde(deserialize_with = "deserialize_export_path")]
    pub path: String,
}

fn deserialize_absolute_container_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    reject_single_quote(&value)?;
    if value.starts_with('/') && !value.trim_end_matches('/').is_empty() {
        Ok(value)
    } else {
        Err(de::Error::custom(
            "container path must be absolute and not root",
        ))
    }
}

fn deserialize_export_path<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    reject_single_quote(&value)?;
    Ok(value)
}

fn reject_single_quote<E>(value: &str) -> Result<(), E>
where
    E: de::Error,
{
    if value.contains('\'') {
        Err(E::custom("path must not contain single quotes"))
    } else {
        Ok(())
    }
}

/// Timers to start scripts periodically in the container
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TimerConfig {
    /// Name
    pub name: String,
    /// Description
    pub description: Option<String>,
    /// Schedule in cron format (standard 5-field).
    #[serde(deserialize_with = "crate::config::deserialize_cron")]
    pub schedule: Cron,
    /// Script to run
    pub script: String,
    /// When true, the timer is not rendered or installed
    #[serde(default)]
    pub disabled: bool,
}

impl AppConfig {
    pub const KIND: &'static str = "app";

    /// Units referenced through `${unit:key}` tokens across `runtime.env` and
    /// every build layer's `env`, deduplicated and sorted.
    pub fn unit_deps(&self) -> BTreeSet<UnitName> {
        let mut deps: BTreeSet<UnitName> = self
            .runtime
            .as_ref()
            .into_iter()
            .flat_map(|runtime| runtime.env.unit_refs().cloned())
            .collect();
        for layer in &self.builds {
            deps.extend(layer.env.unit_refs().cloned());
        }
        deps
    }

    pub fn database_deps(&self, ctx: &MainContext) -> Result<Vec<UnitName>, ReferenceError> {
        let mut dbs = Vec::new();
        for dep in self.unit_deps() {
            match UnitConfig::load(ctx, &dep) {
                Ok(UnitConfig::Db(_)) => dbs.push(dep),
                Ok(_) => continue,
                Err(err) => return Err(ReferenceError::from(err).at(Location::unit(dep.as_str()))),
            }
        }
        Ok(dbs)
    }

    /// The db-server units backing this app's databases, deduped. Serve gates
    /// the app's `dpl start` on these containers being up (`start_after`).
    pub fn db_server_deps(&self, ctx: &MainContext) -> Result<Vec<UnitName>, ReferenceError> {
        let mut servers = BTreeSet::new();
        for db in self.database_deps(ctx)? {
            match UnitConfig::load(ctx, &db) {
                Ok(UnitConfig::Db(config)) => {
                    servers.insert(config.server);
                }
                Ok(_) => continue,
                Err(err) => return Err(ReferenceError::from(err).at(Location::unit(db.as_str()))),
            }
        }
        Ok(servers.into_iter().collect())
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), ReferenceError> {
        if let Some(runtime) = &self.runtime {
            runtime.env.resolve(ctx, "runtime.env")?;
        }

        for (index, layer) in self.builds.iter().enumerate() {
            layer.env.resolve(ctx, &format!("builds[{index}].env"))?;
        }

        for dep in self.unit_deps() {
            match UnitConfig::load(ctx, &dep) {
                Ok(UnitConfig::Db(config)) => config
                    .validate_references(ctx)
                    .map_err(|err| err.at(Location::unit(dep.as_str())))?,
                Ok(_) => continue,
                Err(err) => return Err(ReferenceError::from(err).at(Location::unit(dep.as_str()))),
            }
        }

        Ok(())
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        unit_name: &UnitName,
        key: &str,
    ) -> Result<String, ReferenceError> {
        match key {
            // `http://host:port` for `proxy_pass`.
            "url" => {
                let port = self.runtime_port(key)?;
                Ok(format!("http://{}:{}", unit_name.scoped_unit_name(), port))
            }
            // `host:port` for `uwsgi_pass`, `fastcgi_pass`.
            "socket" => {
                let port = self.runtime_port(key)?;
                Ok(format!("{}:{}", unit_name.scoped_unit_name(), port))
            }
            // Absolute path of this app's static export inside the nginx container.
            "export" => {
                let version = DeployState::get_active_version(ctx, unit_name)
                    .map_err(|_| ReferenceError::not_deployed(unit_name.as_str()))?;
                Ok(format!("{NGINX_WWW_MOUNT}/{unit_name}_{version}"))
            }
            _ => Err(ReferenceError::unknown_export(key)),
        }
    }

    /// The port `url`/`socket` are built from. Absent for a static unit and for
    /// a runtime that declares no port.
    fn runtime_port(&self, key: &str) -> Result<u16, ReferenceError> {
        self.runtime
            .as_ref()
            .and_then(|runtime| runtime.port)
            .ok_or_else(|| ReferenceError::missing_port(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> AppConfig {
        AppConfig {
            image: "alpine".into(),
            builds: Vec::new(),
            runtime: Some(RuntimeConfig {
                port: Some(8080),
                env: EnvList::default(),
                init: None,
                cmd: "./run".into(),
            }),
            volumes: Vec::new(),
            exports: Vec::new(),
            timers: Vec::new(),
        }
    }

    /// A static build-and-export unit: no `runtime`, only `builds`/`exports`.
    fn static_config() -> AppConfig {
        AppConfig {
            image: "alpine".into(),
            builds: Vec::new(),
            runtime: None,
            volumes: Vec::new(),
            exports: Vec::new(),
            timers: Vec::new(),
        }
    }

    #[test]
    fn app_unit_deps_from_env_and_builds() {
        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nruntime:\n  port: 8080\n  cmd: ./run\n  env:\n    DB: \"${app-db:url}\"\n    SECRET: \"${secret:k}\"\nbuilds:\n  - env:\n      API: \"${api:url}\"\n",
        )
        .unwrap();
        // BTreeSet → sorted, deduped, secret ref dropped.
        let deps = config.unit_deps();
        let names: Vec<&str> = deps.iter().map(UnitName::as_str).collect();
        assert_eq!(names, vec!["api", "app-db"]);
    }

    #[test]
    fn app_unit_deps_empty_without_refs() {
        assert!(sample_config().unit_deps().is_empty());
    }

    #[test]
    fn app_database_deps_keeps_db_skips_other_kinds() {
        use tempfile::TempDir;

        // app `foo` references a `db` unit and another `app` unit. Only the db
        // is a startup dependency; the app reference is dropped.
        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nruntime:\n  port: 8080\n  cmd: ./run\n  env:\n    DB: \"${db-x:url}\"\n    UPSTREAM: \"${other-app:url}\"\nbuilds: []\n",
        )
        .unwrap();

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        ctx.write_test_unit(
            "db-x",
            "type: db\nserver: pg-main\nuser: app1\nsecret: db-x-pass\n",
        );
        ctx.write_test_unit(
            "other-app",
            "type: app\nimage: alpine\nbuilds: []\nruntime:\n  port: 9090\n  cmd: ./run\n",
        );
        let deps = config.database_deps(&ctx).unwrap();
        let names: Vec<&str> = deps.iter().map(UnitName::as_str).collect();
        assert_eq!(names, vec!["db-x"]);
    }

    #[test]
    fn db_server_deps_maps_db_to_server() {
        use tempfile::TempDir;

        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nruntime:\n  port: 8080\n  cmd: ./run\n  env:\n    DB: \"${db-x:url}\"\nbuilds: []\n",
        )
        .unwrap();

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        ctx.write_test_unit(
            "db-x",
            "type: db\nserver: pg-main\nuser: app1\nsecret: db-x-pass\n",
        );

        let deps = config.db_server_deps(&ctx).unwrap();
        let servers: Vec<&str> = deps.iter().map(UnitName::as_str).collect();
        assert_eq!(servers, vec!["pg-main"]);
    }

    #[test]
    fn db_server_deps_dedups_shared_server() {
        use tempfile::TempDir;

        // Two databases on the same server collapse to one gate entry.
        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nruntime:\n  port: 8080\n  cmd: ./run\n  env:\n    A: \"${db-a:url}\"\n    B: \"${db-b:url}\"\nbuilds: []\n",
        )
        .unwrap();

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        ctx.write_test_unit(
            "db-a",
            "type: db\nserver: pg-main\nuser: app1\nsecret: db-a-pass\n",
        );
        ctx.write_test_unit(
            "db-b",
            "type: db\nserver: pg-main\nuser: app2\nsecret: db-b-pass\n",
        );

        let deps = config.db_server_deps(&ctx).unwrap();
        let servers: Vec<&str> = deps.iter().map(UnitName::as_str).collect();
        assert_eq!(servers, vec!["pg-main"]);
    }

    #[test]
    fn db_server_deps_empty_without_db_refs() {
        let ctx = MainContext::default();
        assert!(sample_config().db_server_deps(&ctx).unwrap().is_empty());
    }

    #[test]
    fn app_resolve_export_url() {
        let ctx = MainContext::default();
        let config = sample_config();
        assert_eq!(
            config
                .resolve_export(&ctx, &UnitName::new("web").unwrap(), "url")
                .unwrap(),
            "http://dpl--web:8080"
        );
    }

    #[test]
    fn app_resolve_export_socket() {
        let ctx = MainContext::default();
        let config = sample_config();
        assert_eq!(
            config
                .resolve_export(&ctx, &UnitName::new("web").unwrap(), "socket")
                .unwrap(),
            "dpl--web:8080"
        );
    }

    #[test]
    fn app_resolve_export_unknown_key() {
        let ctx = MainContext::default();
        let config = sample_config();
        assert!(
            config
                .resolve_export(&ctx, &UnitName::new("web").unwrap(), "nope")
                .is_err()
        );
    }

    #[test]
    fn app_resolve_export_dir() {
        use std::fs;

        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let state_path = ctx.deploy_state_path(&UnitName::new("web").unwrap());
        fs::create_dir_all(state_path.parent().unwrap()).unwrap();
        fs::write(
            &state_path,
            r#"{"active_version":3,"last_version":3,"last_status":"ready","updated_at":"2026-05-31T07:00:00.000000Z"}"#,
        )
        .unwrap();

        assert_eq!(
            sample_config()
                .resolve_export(&ctx, &UnitName::new("web").unwrap(), "export")
                .unwrap(),
            "/var/www/web_3"
        );
    }

    #[test]
    fn app_resolve_export_not_deployed() {
        use tempfile::TempDir;

        use crate::reference::ReferenceErrorKind;

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        // No deploy state on disk → no active deployment to export from.
        let err = sample_config()
            .resolve_export(&ctx, &UnitName::new("web").unwrap(), "export")
            .unwrap_err();
        assert!(matches!(err.kind, ReferenceErrorKind::NotDeployed { name } if name == "web"));
    }

    #[test]
    fn parse_static_app_without_runtime() {
        // A build-and-export unit: no `runtime`, no `port`.
        let config: UnitConfig = serde_yaml::from_str(
            "type: app\nimage: alpine\nbuilds:\n  - files: [\"*\"]\n    script: npm run build\nexports:\n  - source: /app/dist\n    path: /\n",
        )
        .unwrap();

        let UnitConfig::App(app) = config else {
            panic!("expected app variant");
        };
        assert!(app.runtime.is_none());
        assert!(app.unit_deps().is_empty());
    }

    #[test]
    fn parse_export_rejects_relative_source() {
        let err = serde_yaml::from_str::<UnitConfig>(
            "type: app\nimage: alpine\nbuilds: []\nexports:\n  - source: dist\n    path: /\n",
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("container path must be absolute and not root"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_export_rejects_root_source() {
        let err = serde_yaml::from_str::<UnitConfig>(
            "type: app\nimage: alpine\nbuilds: []\nexports:\n  - source: /\n    path: /\n",
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("container path must be absolute and not root"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_export_rejects_single_quote_in_source() {
        let err = serde_yaml::from_str::<UnitConfig>(
            "type: app\nimage: alpine\nbuilds: []\nexports:\n  - source: /app/it''s-static\n    path: /\n",
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("path must not contain single quotes"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_export_rejects_single_quote_in_path() {
        let err = serde_yaml::from_str::<UnitConfig>(
            "type: app\nimage: alpine\nbuilds: []\nexports:\n  - source: /app/static\n    path: /it''s-static\n",
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("path must not contain single quotes"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_volume_rejects_relative_path() {
        let err = serde_yaml::from_str::<UnitConfig>(
            "type: app\nimage: alpine\nbuilds: []\nvolumes:\n  - source: app-data\n    path: data\nruntime:\n  port: 8080\n  cmd: ./run\n",
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("container path must be absolute and not root"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn parse_volume_rejects_root_path() {
        let err = serde_yaml::from_str::<UnitConfig>(
            "type: app\nimage: alpine\nbuilds: []\nvolumes:\n  - source: app-data\n    path: /\nruntime:\n  port: 8080\n  cmd: ./run\n",
        )
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("container path must be absolute and not root"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn static_app_validate_references_ok() {
        // No runtime env to resolve, no build refs → validation succeeds.
        let ctx = MainContext::default();
        static_config().validate_references(&ctx).unwrap();
    }

    #[test]
    fn static_app_resolve_export_url_and_socket_error() {
        use crate::reference::ReferenceErrorKind;

        let ctx = MainContext::default();
        let config = static_config();
        let name = UnitName::new("web").unwrap();

        // A static unit has no runtime, so the port-backed keys have no port to
        // build from - only `export` is valid.
        for key in ["url", "socket"] {
            let err = config.resolve_export(&ctx, &name, key).unwrap_err();
            assert!(
                matches!(&err.kind, ReferenceErrorKind::MissingPort { key: k } if k == key),
                "expected MissingPort for {key}, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn parse_runtime_without_port() {
        // A worker: a long-running service that listens on nothing.
        let config: UnitConfig = serde_yaml::from_str(
            "type: app\nimage: alpine\nbuilds: []\nruntime:\n  cmd: ./worker\n",
        )
        .unwrap();

        let UnitConfig::App(app) = config else {
            panic!("expected app variant");
        };
        let runtime = app.runtime.expect("runtime present");
        assert_eq!(runtime.port, None);
        assert_eq!(runtime.cmd, "./worker");
    }

    #[test]
    fn portless_runtime_resolve_export_url_and_socket_error() {
        use crate::reference::ReferenceErrorKind;

        // A runtime without a port is reachable by nothing: `url` and `socket`
        // have no port to build from, and say so.
        let mut config = sample_config();
        config.runtime.as_mut().unwrap().port = None;

        let ctx = MainContext::default();
        let name = UnitName::new("worker").unwrap();

        for key in ["url", "socket"] {
            let err = config.resolve_export(&ctx, &name, key).unwrap_err();
            assert!(
                matches!(&err.kind, ReferenceErrorKind::MissingPort { key: k } if k == key),
                "expected MissingPort for {key}, got {:?}",
                err.kind
            );
        }
    }

    #[test]
    fn portless_runtime_env_refs_still_resolve() {
        // Dropping the port must not change dependency discovery.
        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nbuilds: []\nruntime:\n  cmd: ./worker\n  env:\n    DB: \"${db-x:url}\"\n",
        )
        .unwrap();

        let deps = config.unit_deps();
        let names: Vec<&str> = deps.iter().map(UnitName::as_str).collect();
        assert_eq!(names, vec!["db-x"]);
    }
}
