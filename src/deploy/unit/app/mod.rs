mod artifacts;
mod model;
mod podman;

use std::{
    collections::BTreeMap,
    fs,
    io::{
        self,
        Read,
    },
    path::Path,
    time::Duration,
};

use chrono::Utc;
use croner::Cron;
use tempfile::TempDir;

pub use self::model::AppConfig;
use self::{
    artifacts::ArtifactsContext,
    podman::{
        PodmanContext,
        export_destination,
    },
};
use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        RunError,
        UnitConfig,
        unit::{
            deployed_version,
            domain::{
                DomainConfig,
                DomainUnit,
            },
            list_units,
            podman_run_with_env,
            undeploy_container,
            wait_ready,
        },
    },
    log,
    podman::{
        REPORT_TIMEOUT,
        copy_image_dir_to_host,
    },
    state::DeployState,
    timers::TimersState,
};

/// How long `dpl start` waits for each database dependency before giving up.
const DB_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub struct AppUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: AppConfig,
}

impl<'a> AppUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a UnitName, config: AppConfig) -> Self {
        Self { ctx, name, config }
    }

    fn prepare<R: Read>(&self, archive: R) -> Result<TempDir, DeployError> {
        log::phase("preparing");

        let state_dir = self.ctx.state_dir();
        let temp_dir = tempfile::tempdir_in(&state_dir)
            .map_err(|e| DeployError::step_prepare("create temporary directory", e))?;
        let deploy_dir = temp_dir.path();

        let archive_path = deploy_dir.join("app.tar.gz");
        save_archive(archive, &archive_path).map_err(|e| {
            DeployError::step_prepare("save app archive to the temporary directory", e)
        })?;

        let artifacts = ArtifactsContext {
            config: &self.config,
        };

        artifacts
            .save(deploy_dir)
            .map_err(|e| DeployError::step_prepare("render artifacts", e))?;

        Ok(temp_dir)
    }

    /// Run the install phase: extract archive, render artifacts,
    /// build image, install service.
    pub fn deploy<R: Read>(
        self,
        state: &mut DeployState,
        version: u32,
        archive: R,
    ) -> Result<(), DeployError> {
        let temp_dir = self.prepare(archive)?;
        self.deploy_worker(version, temp_dir.path(), state)?;

        self.start_via_serve(state)?;
        self.redeploy_dependent_domains();

        Ok(())
    }

    fn deploy_worker(
        &self,
        version: u32,
        deploy_dir: &Path,
        state: &mut DeployState,
    ) -> Result<(), DeployError> {
        self.build_inner(deploy_dir, version)?;

        if let Some(active_version) = state.take_active_version() {
            log::phase(format_args!("uninstalling v{active_version}"));
            Self::undeploy(self.ctx, self.name, active_version);
        }

        if let Err(err) = self.install_inner(deploy_dir, version) {
            Self::undeploy(self.ctx, self.name, version);
            return Err(err);
        }

        Ok(())
    }

    /// Domain units whose routes reference this app via `${<app>:export|url}`.
    fn dependent_domains(&self) -> Vec<(UnitName, DomainConfig)> {
        list_units(self.ctx, |c| matches!(c, UnitConfig::Domain(_)))
            .into_iter()
            .filter_map(|(name, config)| match config {
                UnitConfig::Domain(d) if d.unit_deps().contains(self.name) => Some((name, d)),
                _ => None,
            })
            .collect()
    }

    /// Re-render and reload every domain that depends on this app.
    /// Should be called after state saved.
    fn redeploy_dependent_domains(&self) {
        let domains = self.dependent_domains();
        if domains.is_empty() {
            return;
        }

        log::phase("updating dependent domains");
        for (name, config) in domains {
            let (_guard, mut state) = match DeployState::acquire(self.ctx, &name) {
                Ok(v) => v,
                Err(err) => {
                    log::error(format!("skip domain '{name}': {err}"));
                    continue;
                }
            };

            if let Err(err) = state.begin_deploy(DomainConfig::KIND) {
                log::error(format!("skip domain '{name}': {err}"));
                continue;
            }

            let domain = DomainUnit::new(self.ctx, &name, config);
            if let Err(err) = domain.deploy(&mut state) {
                if let Some((stage, message)) = err.failure() {
                    state.set_failed(stage, message);
                }
                log::error(format!(
                    "domain '{name}' redeploy failed: {:#}",
                    anyhow::Error::new(err)
                ));
            }
        }
    }

    fn build_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        {
            log::phase("extracting app archive");
            let archive_path = deploy_dir.join("app.tar.gz");
            let app_dir = deploy_dir.join("app");
            crate::archive::extract(&archive_path, &app_dir)
                .map_err(|e| DeployError::step_build("extract app archive", e))?;
        }

        log::phase("building app image");
        let build_env = self.resolve_build_env()?;
        PodmanContext::new(self.name, version)
            .build(deploy_dir, &self.ctx.build_log_path(self.name), &build_env)
            .map_err(|e| DeployError::step_build("build app image", e))?;

        Ok(())
    }

    fn resolve_build_env(&self) -> Result<BTreeMap<String, String>, DeployError> {
        let mut build_env = BTreeMap::new();

        for (index, layer) in self.config.builds.iter().enumerate() {
            if layer.script.is_none() {
                continue;
            }

            let prefix = format!("builds[{index}].env");
            let layer_env = layer
                .env
                .resolve(self.ctx, &prefix)
                .map_err(|e| DeployError::step_build("resolve build env", e))?;

            for (name, value) in layer_env {
                build_env.insert(name, value);
            }
        }

        Ok(build_env)
    }

    fn install_inner(&self, _deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        // A static export-only unit has no runtime.
        // Files are copied into a server's www dir later, at domain deploy.
        if self.config.runtime.is_none() {
            return Ok(());
        }

        log::phase("registering runtime environment");
        self.register_runtime_env(version)?;

        log::phase("registering timers");
        self.register_timers();

        Ok(())
    }

    fn register_runtime_env(&self, version: u32) -> Result<(), DeployError> {
        let Some(runtime) = &self.config.runtime else {
            return Ok(());
        };

        let env = runtime
            .env
            .resolve(self.ctx, "runtime.env")
            .map_err(|e| DeployError::step_install("resolve runtime env", e))?;

        if !env.is_empty() {
            crate::podman::env::save(self.ctx, self.name, version, &env)
                .map_err(|e| DeployError::step_install("save runtime env", e))?;
        }

        Ok(())
    }

    /// Hand the container off to `dpl serve` and wait for it to come
    /// up. Marks the build `Check` (active, unverified), nudges serve to start
    /// it, then runs the readiness check and flips to `Ready`. A static unit has
    /// no container and is `Ready` immediately.
    fn start_via_serve(&self, state: &mut DeployState) -> Result<(), DeployError> {
        let Some(runtime) = &self.config.runtime else {
            state.set_ready();
            return Ok(());
        };

        // Anything else of this unit still around (a stop an earlier deploy
        // missed) would share the DNS alias with the container about to start.
        let container = self.name.container_name(state.last_version);
        evict_strays(self.name, &container);

        let db_servers = self
            .config
            .db_server_deps(self.ctx)
            .map_err(|e| DeployError::step_prepare("resolve db-server dependencies", e))?;
        state.set_start_after(db_servers);
        state.set_check();

        crate::serve::notify_or_warn(self.ctx, &container);

        wait_ready(self.name, AppConfig::KIND, &container, runtime.port)?;
        state.set_ready();
        Ok(())
    }

    /// Reconcile persisted timer state against the configured timers.
    /// Best-effort: a failure here must not fail an otherwise-successful deploy.
    fn register_timers(&self) {
        let configured: Vec<(String, Cron)> = self
            .config
            .timers
            .iter()
            .filter(|t| !t.disabled)
            .map(|t| (t.name.clone(), t.schedule.clone()))
            .collect();

        let (_lock, mut timers) = match TimersState::acquire(self.ctx, self.name) {
            Ok(acquired) => acquired,
            Err(err) => {
                log::warn(format!("register timers for '{}': {err}", self.name));
                return;
            }
        };

        timers.reconcile(&configured, Utc::now());
    }

    pub fn undeploy(ctx: &MainContext, name: &UnitName, version: u32) {
        // Stop the running container before dropping its image: the versioned
        // one, then a legacy-named one left by an older dpl (plus the env file).
        if let Err(err) = crate::podman::stop_and_remove(&name.container_name(version)) {
            log::warn(format!("stop container '{name}' v{version}: {err}"));
        }
        undeploy_container(ctx, name, version);
        PodmanContext::new(name, version).remove();
    }

    /// `active` is the version in service: its container is reported, or a
    /// legacy-named one still running from before the upgrade.
    pub fn inspect(&self, active: u32) -> Result<(), DeployError> {
        // A static build-and-export unit never runs a container.
        if self.config.runtime.is_none() {
            log::print_field(
                "Container",
                format!("{} static export (no container)", log::success_mark()),
            );
            return Ok(());
        }

        // A probe failure is reported by print_container_state itself.
        let container = running_container(self.name, active, REPORT_TIMEOUT)
            .ok()
            .flatten()
            .unwrap_or_else(|| self.name.container_name(active));
        crate::podman::inspect::print_container_state(&container);

        Ok(())
    }

    /// Run the app container in the foreground. `version` overrides the
    /// deployed one (serve passes the slot it supervises).
    pub fn start(&self, version: Option<u32>) -> Result<(), RunError> {
        if self.config.runtime.is_none() {
            return Err(RunError::new(
                format!("app '{}' is a static unit with no container", self.name),
                io::Error::other("nothing to start"),
            ));
        }

        let version = match version {
            Some(version) => version,
            None => deployed_version(self.ctx, self.name, AppConfig::KIND)?,
        };
        let container = self.name.container_name(version);
        let mut cmd = podman_run_with_env(self.ctx, self.name, &container, version)?;
        // Stable DNS name across versions: `${app:url}` resolves to it.
        cmd.network_alias(&self.name.scoped_unit_name());

        let databases = self
            .config
            .database_deps(self.ctx)
            .map_err(|e| RunError::new("resolve database dependencies", e))?;

        for db in databases {
            crate::deploy::unit::db::wait_until_ready(self.ctx, &db, DB_WAIT_TIMEOUT)
                .map_err(|e| RunError::new(format!("wait for database '{db}'"), e))?;
        }

        for volume in &self.config.volumes {
            cmd.volume(&volume.source, &volume.path, &[]);
        }

        let image = format!("localhost/{}:{}", self.name, version);
        cmd.run_foreground(image, &self.ctx.runtime_log_path(self.name))
            .map_err(|e| RunError::new("run podman foreground", e))
    }
}

fn save_archive<R: Read>(archive: R, dst: &Path) -> io::Result<()> {
    let mut reader = archive;
    let mut archive_file = fs::File::create(dst)?;
    io::copy(&mut reader, &mut archive_file)?;
    Ok(())
}

/// Whether deploy state's `kind` names an app. A missing kind (a state file
/// written before `kind` existed, so its container carries the legacy name)
/// counts as one: the version argv and the legacy fallback this enables are
/// harmless for every other kind.
pub fn is_app_kind(kind: Option<&str>) -> bool {
    kind.is_none_or(|k| k == AppConfig::KIND)
}

/// The names an app's container of `version` may run under, most specific
/// first: `dpl--{unit}--{version}`, then the legacy `dpl--{unit}` of a
/// container started by an older dpl (adopted until its next deploy).
pub fn container_names(name: &UnitName, version: u32) -> [String; 2] {
    [name.container_name(version), name.scoped_unit_name()]
}

/// The names a supervised unit's container may run under, as deploy state
/// describes the unit: [`container_names`] for an app with an active version,
/// the legacy `dpl--{unit}` alone for everything else.
pub fn probe_names(name: &UnitName, kind: Option<&str>, active: Option<u32>) -> Vec<String> {
    match active {
        Some(version) if is_app_kind(kind) => container_names(name, version).to_vec(),
        _ => vec![name.scoped_unit_name()],
    }
}

/// Stop and remove every container of `name` other than `keep`. A stop an
/// earlier deploy missed (interrupted, or podman did not answer) leaves a
/// container that shares the DNS alias with the one about to start, and
/// nothing else revisits it: `--replace` only evicts the same name. Best
/// effort - the next deploy runs it again.
fn evict_strays(name: &UnitName, keep: &str) {
    let filter = format!("name={}", name.scoped_unit_name());
    let args = ["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"];
    let listed = match crate::podman::run_podman(&args) {
        Ok(out) => out,
        Err(err) => {
            log::warn(format!("list containers of '{name}': {err}"));
            return;
        }
    };
    for stray in stray_containers(name, keep, &listed) {
        log::warn(format!("removing stray container '{stray}'"));
        if let Err(err) = crate::podman::stop_and_remove(stray) {
            log::warn(format!("stop container '{stray}': {err}"));
        }
    }
}

/// The containers of `name` among `ps_names` (one per line) other than `keep`:
/// the legacy `dpl--{unit}` or any `dpl--{unit}--{version}`. Exact, since the
/// `podman ps` name filter is a substring match that also returns other units
/// sharing the prefix.
fn stray_containers<'a>(name: &UnitName, keep: &str, ps_names: &'a str) -> Vec<&'a str> {
    let legacy = name.scoped_unit_name();
    let prefix = format!("{legacy}--");
    ps_names
        .lines()
        .map(str::trim)
        .filter(|n| *n != keep)
        .filter(|n| {
            *n == legacy
                || n.strip_prefix(&prefix)
                    .is_some_and(|v| v.parse::<u32>().is_ok())
        })
        .collect()
}

/// The first of [`container_names`] that podman reports running. `Err` only
/// when podman did not answer (the first probe to fail ends the search, so a
/// wedged podman costs one `limit`, not two).
pub fn running_container(
    name: &UnitName,
    version: u32,
    limit: Duration,
) -> io::Result<Option<String>> {
    for container in container_names(name, version) {
        if crate::podman::is_running_within(&container, limit)? {
            return Ok(Some(container));
        }
    }
    Ok(None)
}

/// Copy `app`'s static exports (its active version) into `www_dir`
/// Wupe any prior `<app>_*` there.
pub fn export_to_www(ctx: &MainContext, app: &UnitName, www_dir: &Path) -> Result<(), DeployError> {
    let UnitConfig::App(config) = UnitConfig::load(ctx, app)
        .map_err(|e| DeployError::step_prepare(format!("load app '{app}'"), e))?
    else {
        return Ok(());
    };

    if config.exports.is_empty() {
        return Ok(());
    }

    let version = DeployState::get_active_version(ctx, app)
        .map_err(|e| DeployError::step_prepare(format!("active version for app '{app}'"), e))?;
    let image = format!("localhost/{app}:{version}");
    let version_dir = format!("{app}_{version}");

    wipe_app_dirs(www_dir, app);

    for export in &config.exports {
        let src = export.source.trim_end_matches('/');
        let dest = www_dir.join(export_destination(&version_dir, &export.path));
        copy_image_dir_to_host(&image, src, &dest)
            .map_err(|e| DeployError::step_install(format!("export app '{app}' to www"), e))?;
    }

    Ok(())
}

/// Remove every `<app>_*` export directly under `www_dir`.
fn wipe_app_dirs(www_dir: &Path, app: &UnitName) {
    let Ok(entries) = fs::read_dir(www_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_str().is_some_and(|n| is_app_export_dir(n, app))
            && let Err(err) = fs::remove_dir_all(entry.path())
        {
            log::warn(format!(
                "remove stale export '{}': {err}",
                entry.path().display()
            ));
        }
    }
}

fn is_app_export_dir(name: &str, app: &UnitName) -> bool {
    name.strip_prefix(app.as_str())
        .and_then(|rest| rest.strip_prefix('_'))
        .is_some_and(|version| !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn write_unit(base: &Path, name: &str, config: &str) {
        MainContext {
            base: base.to_path_buf(),
            master_key: None,
        }
        .write_test_unit(name, config);
    }

    #[test]
    fn dependent_domains_selects_only_referencing_domains() {
        let base = TempDir::new().unwrap();

        // Serves this app's static export - should be selected.
        write_unit(
            base.path(),
            "site",
            "type: domain\nserver: nginx\nhosts: [\"example.com\"]\nroutes:\n  - location: /\n    kind: serve_files\n    root: \"${web:export}\"\n",
        );
        // References a different app - should be skipped.
        write_unit(
            base.path(),
            "other-site",
            "type: domain\nserver: nginx\nhosts: [\"other.com\"]\nroutes:\n  - location: /\n    kind: reverse_proxy\n    target: \"${api:url}\"\n",
        );
        // A non-domain unit - must not match the domain predicate.
        write_unit(
            base.path(),
            "web-db",
            "type: db\nserver: pg-main\nuser: app1\nsecret: app1-pass\n",
        );

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config: AppConfig = serde_yaml::from_str(
            "image: alpine\nbuilds: []\nruntime:\n  port: 8080\n  cmd: ./run\n",
        )
        .unwrap();
        let unit_name = UnitName::new("web").unwrap();
        let app = AppUnit::new(&ctx, &unit_name, config);

        let domains = app.dependent_domains();
        let names: Vec<&str> = domains.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, vec!["site"]);
    }

    #[test]
    fn is_app_kind_treats_missing_kind_as_app() {
        assert!(is_app_kind(Some("app")));
        assert!(is_app_kind(None));
        assert!(!is_app_kind(Some("db-server")));
    }

    #[test]
    fn container_names_versioned_first_then_legacy() {
        let app = UnitName::new("web").unwrap();
        assert_eq!(container_names(&app, 3), ["dpl--web--3", "dpl--web"]);
    }

    #[test]
    fn probe_names_follow_kind_and_active_version() {
        let unit = UnitName::new("web").unwrap();
        let both = vec!["dpl--web--3".to_string(), "dpl--web".to_string()];
        let legacy = vec!["dpl--web".to_string()];
        assert_eq!(probe_names(&unit, Some("app"), Some(3)), both);
        assert_eq!(probe_names(&unit, None, Some(3)), both);
        assert_eq!(probe_names(&unit, Some("app"), None), legacy);
        assert_eq!(probe_names(&unit, Some("db-server"), Some(3)), legacy);
    }

    #[test]
    fn stray_containers_are_the_units_other_names_only() {
        let app = UnitName::new("web").unwrap();
        // What `podman ps --filter name=dpl--web` returns: every name
        // containing the substring, other units included.
        let ps = "dpl--web--3\ndpl--web--1\ndpl--web\ndpl--web-api\n\
                  dpl--web-api--2\ndpl--webapp--2\ndpl--web--x\n";
        assert_eq!(
            stray_containers(&app, "dpl--web--3", ps),
            ["dpl--web--1", "dpl--web"]
        );
    }

    #[test]
    fn is_app_export_dir_matches_versioned_dirs_only() {
        let app = UnitName::new("blog").unwrap();
        assert!(is_app_export_dir("blog_3", &app));
        assert!(is_app_export_dir("blog_12", &app));
        assert!(!is_app_export_dir("blog", &app)); // no version suffix
        assert!(!is_app_export_dir("blog_", &app)); // empty version
        assert!(!is_app_export_dir("blog_v2", &app)); // non-numeric version
        assert!(!is_app_export_dir("blogger_1", &app)); // different app, shared prefix
    }

    #[test]
    fn wipe_app_dirs_removes_only_matching_app() {
        let dir = TempDir::new().unwrap();
        let www = dir.path();
        let app = UnitName::new("blog").unwrap();
        fs::create_dir_all(www.join("blog_1/static")).unwrap();
        fs::create_dir_all(www.join("blog_2")).unwrap();
        fs::create_dir_all(www.join("blogger_1")).unwrap();
        fs::create_dir_all(www.join("shop_3")).unwrap();

        wipe_app_dirs(www, &app);

        assert!(!www.join("blog_1").exists());
        assert!(!www.join("blog_2").exists());
        assert!(www.join("blogger_1").exists()); // shared-prefix app untouched
        assert!(www.join("shop_3").exists()); // other app untouched
    }
}
