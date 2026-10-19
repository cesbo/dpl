mod artifacts;
mod expect;
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
    model::DeployStrategy,
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
            deployed_version_of,
            domain::{
                DomainConfig,
                DomainUnit,
            },
            list_units,
            podman_run_with_env,
            wait_ready,
        },
    },
    log,
    podman::{
        REPORT_TIMEOUT,
        copy_image_dir_to_host,
    },
    state::{
        DeployState,
        DeployStatus,
    },
    timers::TimersState,
};

/// How long `dpl start` waits for each database dependency before giving up.
const DB_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// Ready versions besides the active one that keep their image and env.
// ponytail: fixed retention; a yaml field when someone needs more
const KEEP: usize = 1;

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
        self.deploy_worker(version, temp_dir.path())?;
        self.hand_off(state, version)
    }

    /// Bring the retained `source` back as `version` through the deploy
    /// hand-off: retag its image, re-encrypt the env it ran with under the
    /// new version (rather than copying the file), then hand it off per the
    /// strategy. A startup failure propagates like a deploy's; the next
    /// deploy or rollback discards the retag and env.
    pub fn rollback(
        &self,
        state: &mut DeployState,
        version: u32,
        source: u32,
    ) -> Result<(), DeployError> {
        log::phase(format_args!("retagging v{source} as v{version}"));
        crate::podman::tag_image(
            &format!("localhost/{}:{source}", self.name),
            &format!("localhost/{}:{version}", self.name),
        )
        .map_err(|e| DeployError::step_prepare(format!("tag v{source} as v{version}"), e))?;

        log::phase("re-encrypting runtime env");
        if let Err(err) = self.reencrypt_runtime_env(source, version) {
            // Best effort: the next deploy or rollback discards it anyway.
            let _ = PodmanContext::new(self.name, version).remove_image();
            return Err(err);
        }

        // Timers come from the current yaml, as at deploy.
        if self.config.runtime.is_some() {
            log::phase("registering timers");
            self.register_timers();
        }

        self.hand_off(state, version)
    }

    /// Encrypt the env `source` ran with under `version`. A missing file is an
    /// empty env (`register_runtime_env` writes none for one), not an error.
    fn reencrypt_runtime_env(&self, source: u32, version: u32) -> Result<(), DeployError> {
        let env = crate::podman::env::load(self.ctx, self.name, source)
            .map_err(|e| DeployError::step_install(format!("load runtime env of v{source}"), e))?;

        if !env.is_empty() {
            crate::podman::env::save(self.ctx, self.name, version, &env)
                .map_err(|e| DeployError::step_install("save runtime env", e))?;
        }

        Ok(())
    }

    fn deploy_worker(&self, version: u32, deploy_dir: &Path) -> Result<(), DeployError> {
        self.build_inner(deploy_dir, version)?;

        if let Err(err) = self.install_inner(deploy_dir, version) {
            // No container of `version` exists before the hand-off, and the
            // legacy `dpl--{unit}` may be the still-running active version:
            // only the image and env go.
            Self::remove_version(self.ctx, self.name, version);
            crate::podman::prune_images();
            return Err(err);
        }

        Ok(())
    }

    /// Bring `version` into service: stop the active version first
    /// (`replace`), or start it next to the active one and switch once it is
    /// ready (`parallel`, runtime units only).
    fn hand_off(&self, state: &mut DeployState, version: u32) -> Result<(), DeployError> {
        match (self.config.strategy, &self.config.runtime) {
            (DeployStrategy::Parallel, Some(runtime)) => {
                self.hand_off_parallel(state, version, runtime.port)
            }
            _ => self.hand_off_replace(state, version),
        }
    }

    fn hand_off_replace(&self, state: &mut DeployState, version: u32) -> Result<(), DeployError> {
        // Only stopped: its image and env are retained for a rollback.
        if let Some(old) = state.take_active_version() {
            log::phase(format_args!("stopping v{old}"));
            Self::stop_version(self.name, old);
        }

        self.start_via_serve(state)?;
        self.redeploy_dependent_domains();
        Self::prune_versions(self.ctx, self.name, version);

        Ok(())
    }

    /// The new version runs as a candidate (no DNS alias, no `${app:url}`
    /// traffic) next to the active one until it is ready. The switch attaches
    /// the alias, promotes it in state and re-renders the dependent domains
    /// (pinned to the container); only then is the old version stopped. A
    /// readiness or alias failure drops the candidate and leaves the old
    /// version in service; a domain failure reverts to it.
    fn hand_off_parallel(
        &self,
        state: &mut DeployState,
        version: u32,
        port: Option<u16>,
    ) -> Result<(), DeployError> {
        let name = self.name;
        let container = name.container_name(version);
        evict_strays(name, &parallel_keep(name, state.active_version, version));

        let db_servers = self
            .config
            .db_server_deps(self.ctx)
            .map_err(|e| DeployError::step_prepare("resolve db-server dependencies", e))?;
        state.set_start_after(db_servers);
        state.set_candidate();

        crate::serve::notify_or_warn(self.ctx, &container);

        if let Err(err) = wait_ready(name, AppConfig::KIND, &container, port) {
            self.fail_candidate(state, version);
            return Err(err);
        }

        log::phase(format_args!("attaching alias to v{version}"));
        let alias = name.scoped_unit_name();
        if let Err(err) = crate::podman::network_disconnect(crate::podman::NETWORK, &container)
            .and_then(|()| {
                crate::podman::network_connect_alias(crate::podman::NETWORK, &container, &alias)
            })
        {
            self.fail_candidate(state, version);
            return Err(DeployError::step_startup(
                format!("attach alias to v{version}"),
                err,
            ));
        }

        log::phase(format_args!("switching to v{version}"));
        let old = state.promote_candidate();
        if let Err(err) = self.ctx.link_runtime_log(name, version) {
            log::warn(format!("link runtime log of '{name}': {err}"));
        }

        if !self.redeploy_dependent_domains() {
            // nginx must point back at the old container before the
            // candidate stops.
            state.revert_candidate(old);
            self.redeploy_dependent_domains();
            if let Some(old) = old
                && let Err(err) = self.ctx.link_runtime_log(name, old)
            {
                log::warn(format!("link runtime log of '{name}': {err}"));
            }
            stop_container(&container);
            return Err(DeployError::step_startup(
                "switch dependent domains",
                io::Error::other("a dependent domain failed to redeploy"),
            ));
        }

        if let Some(old) = old {
            log::phase(format_args!("stopping v{old}"));
            Self::stop_version(name, old);
        }

        state.set_ready();
        Self::prune_versions(self.ctx, name, version);

        Ok(())
    }

    /// Drop a candidate that never reached its switch: its container only
    /// (the legacy `dpl--{unit}` belongs to the active version). The CLI
    /// marks the deploy failed; the next one discards its image and env.
    fn fail_candidate(&self, state: &mut DeployState, version: u32) {
        stop_container(&self.name.container_name(version));
        state.clear_candidate();
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
    /// Should be called after state saved. True when every one deployed.
    fn redeploy_dependent_domains(&self) -> bool {
        let domains = self.dependent_domains();
        if domains.is_empty() {
            return true;
        }

        log::phase("updating dependent domains");
        let mut all = true;
        for (name, config) in domains {
            let (_guard, mut state) = match DeployState::acquire(self.ctx, &name) {
                Ok(v) => v,
                Err(err) => {
                    log::error(format!("skip domain '{name}': {err}"));
                    all = false;
                    continue;
                }
            };

            if let Err(err) = state.begin_deploy(DomainConfig::KIND) {
                log::error(format!("skip domain '{name}': {err}"));
                all = false;
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
                all = false;
            }
        }
        all
    }

    fn build_inner(&self, deploy_dir: &Path, version: u32) -> Result<(), DeployError> {
        let app_dir = deploy_dir.join("app");
        log::phase("extracting app archive");
        let archive_path = deploy_dir.join("app.tar.gz");
        crate::archive::extract(&archive_path, &app_dir)
            .map_err(|e| DeployError::step_build("extract app archive", e))?;

        // Before the image build, so a miss fails the deploy and leaves the
        // running version untouched. Rollback does not come through here.
        if let Some(expect) = &self.config.expect {
            log::phase(format_args!("checking {}", expect.path));
            expect
                .check(&app_dir)
                .map_err(|e| DeployError::step_build("check archive", e))?;
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
        evict_strays(self.name, std::slice::from_ref(&container));

        let db_servers = self
            .config
            .db_server_deps(self.ctx)
            .map_err(|e| DeployError::step_prepare("resolve db-server dependencies", e))?;
        state.set_start_after(db_servers);
        state.set_check();

        crate::serve::notify_or_warn(self.ctx, &container);

        wait_ready(self.name, AppConfig::KIND, &container, runtime.port)?;
        state.set_ready();

        if let Err(err) = self.ctx.link_runtime_log(self.name, state.last_version) {
            log::warn(format!("link runtime log of '{}': {err}", self.name));
        }
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

    /// Tear down `version` (what `dpl undeploy` removes): its container, env
    /// and image. Retained older versions stay until the next successful
    /// deploy prunes them.
    pub fn undeploy(ctx: &MainContext, name: &UnitName, version: u32) {
        Self::stop_version(name, version);
        Self::remove_version(ctx, name, version);
        crate::podman::prune_images();
    }

    /// Stop and remove the containers `version` may run under: the versioned
    /// one, then the legacy `dpl--{unit}` of an older dpl. Env and image stay.
    fn stop_version(name: &UnitName, version: u32) {
        for container in container_names(name, version) {
            stop_container(&container);
        }
    }

    /// Drop a version's image tag, then its env file. A tag podman did not
    /// drop keeps its env, so the version stays a whole rollback source.
    /// Layers another tag shares stay; the caller prunes once per pass.
    fn remove_version(ctx: &MainContext, name: &UnitName, version: u32) {
        match PodmanContext::new(name, version).remove_image() {
            Ok(()) => remove_env(ctx, name, version),
            Err(err) => log::warn(format!("remove image of '{name} v{version}': {err}")),
        }
    }

    /// Drop every retained version [`keep_versions`] does not keep next to
    /// `new`, then prune dangling layers once. Runs after `new` is ready.
    fn prune_versions(ctx: &MainContext, name: &UnitName, new: u32) {
        let tags = match crate::podman::image_tags(&format!("localhost/{name}")) {
            Ok(tags) => numeric_tags(&tags),
            Err(err) => {
                log::warn(format!("list images of '{name}': {err}"));
                return;
            }
        };

        let keep = keep_versions(&tags, new);
        for version in tags.into_iter().filter(|v| !keep.contains(v)) {
            log::phase(format_args!("removing v{version}"));
            Self::remove_version(ctx, name, version);
            remove_runtime_log(ctx, name, version);
        }
        crate::podman::prune_images();
    }

    /// Discard the last version when it never reached ready: a startup
    /// failure, a deploy interrupted in its readiness wait or its build, a
    /// candidate that never reached its switch. Keeps the invariant that a
    /// tagged `localhost/{unit}:{N}` is a ready version (or the one being
    /// deployed). Warn-only; a no-op after `dpl undeploy`.
    pub fn discard_unready(ctx: &MainContext, name: &UnitName, state: &mut DeployState) {
        let version = state.last_version;
        if version == 0 || state.last_status == DeployStatus::Ready {
            return;
        }

        let tagged = crate::podman::image_exists(&format!("localhost/{name}:{version}"));
        if tagged || state.active_version == Some(version) {
            log::warn(format!("discarding unready v{version}"));
        }

        // The legacy `dpl--{unit}` is the live version's when that is another
        // one (a failure before the hand-off never took it over). Otherwise
        // nothing of the unit may run once its active version is taken: a
        // parallel switch interrupted before "stopping v{old}" left the
        // previous container up, alias attached, and state no longer names it.
        if state.active_version.is_some_and(|active| active != version) {
            stop_container(&name.container_name(version));
        } else {
            Self::stop_version(name, version);
            evict_strays(name, &[]);
        }
        // Untag first, env only once the tag is gone (as `remove_version`);
        // untagging a version that is already gone is a harmless error.
        match PodmanContext::new(name, version).remove_image() {
            Ok(()) => remove_env(ctx, name, version),
            Err(err) if tagged => {
                log::warn(format!("remove image of '{name} v{version}': {err}"));
            }
            Err(_) => {}
        }
        remove_runtime_log(ctx, name, version);
        if state.active_version == Some(version) {
            state.take_active_version();
        }
        if state.candidate_version == Some(version) {
            state.clear_candidate();
        }
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

        let state = DeployState::load(self.ctx, self.name)
            .map_err(|e| RunError::new(format!("load state for '{}'", self.name), e))?;
        let version = match version {
            Some(version) => version,
            None => deployed_version_of(&state, self.name, AppConfig::KIND)?,
        };
        // A candidate (a parallel deploy before its switch) runs next to the
        // active version: no `${app:url}` traffic and no `runtime.log` until
        // the switch, which attaches the alias and moves the link.
        let candidate = state.candidate_version == Some(version);
        let container = self.name.container_name(version);
        let mut cmd = podman_run_with_env(self.ctx, self.name, &container, version)?;
        if !candidate {
            // Stable DNS name across versions: `${app:url}` resolves to it.
            cmd.network_alias(&self.name.scoped_unit_name());
        }

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

        if !candidate {
            adopt_runtime_log(self.ctx, self.name, version);
        }
        let image = format!("localhost/{}:{}", self.name, version);
        cmd.run_foreground(
            image,
            &self.ctx.runtime_log_version_path(self.name, version),
        )
        .map_err(|e| RunError::new("run podman foreground", e))
    }
}

fn save_archive<R: Read>(archive: R, dst: &Path) -> io::Result<()> {
    let mut reader = archive;
    let mut archive_file = fs::File::create(dst)?;
    io::copy(&mut reader, &mut archive_file)?;
    Ok(())
}

fn stop_container(container: &str) {
    if let Err(err) = crate::podman::stop_and_remove(container) {
        log::warn(format!("stop container '{container}': {err}"));
    }
}

fn remove_env(ctx: &MainContext, name: &UnitName, version: u32) {
    if let Err(err) = crate::podman::env::remove(ctx, name, version) {
        log::warn(format!(
            "remove runtime env file for '{name} v{version}': {err}"
        ));
    }
}

/// Point `runtime.log` at this version's file before its first byte, so a
/// restart by the new serve keeps it live. A live link is the deploy's: it
/// moves once a version is in service. A dangling one (its version was
/// discarded) is re-pointed. A regular file is an older dpl's log: it becomes
/// this version's, rotation ring included. Warn-only.
fn adopt_runtime_log(ctx: &MainContext, name: &UnitName, version: u32) {
    let link = ctx.runtime_log_path(name);
    if link.is_symlink() && link.exists() {
        return;
    }
    if link.is_file()
        && let Err(err) =
            log::cri_log::rename_all(&link, ctx.runtime_log_version_path(name, version))
    {
        // The bytes stay where they are; the next start tries again.
        log::warn(format!("adopt runtime log of '{name}': {err}"));
        return;
    }
    if let Err(err) = ctx.link_runtime_log(name, version) {
        log::warn(format!("link runtime log of '{name}': {err}"));
    }
}

/// Drop a version's runtime log with its rotation ring (a missing one is
/// fine). Not for `undeploy`, which keeps logs.
fn remove_runtime_log(ctx: &MainContext, name: &UnitName, version: u32) {
    if let Err(err) = log::cri_log::remove_all(ctx.runtime_log_version_path(name, version)) {
        log::warn(format!("remove runtime log of '{name} v{version}': {err}"));
    }
}

/// The versions among an image repository's tags, highest first. Anything
/// else tagged there is skipped.
pub fn numeric_tags(tags: &[String]) -> Vec<u32> {
    let mut versions: Vec<u32> = tags
        .iter()
        .filter(|t| t.bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|t| t.parse().ok())
        .collect();
    versions.sort_unstable_by(|a, b| b.cmp(a));
    versions
}

/// The versions to keep once `new` is ready: `new` itself and the [`KEEP`]
/// highest others.
pub fn keep_versions(tags: &[u32], new: u32) -> Vec<u32> {
    let mut keep: Vec<u32> = tags.iter().copied().filter(|&t| t != new).collect();
    keep.sort_unstable_by(|a, b| b.cmp(a));
    keep.truncate(KEEP);
    keep.insert(0, new);
    keep
}

/// The retained version a rollback goes to: `wanted` when it is retained and
/// not active, else the highest retained one other than the active.
pub fn rollback_target(
    tags: &[u32],
    active: Option<u32>,
    wanted: Option<u32>,
) -> Result<u32, String> {
    match wanted {
        Some(v) if !tags.contains(&v) => {
            let retained: Vec<String> = tags.iter().map(u32::to_string).collect();
            Err(format!(
                "version {v} is not retained (retained: {})",
                retained.join(", ")
            ))
        }
        Some(v) if active == Some(v) => Err(format!("version {v} is already active")),
        Some(v) => Ok(v),
        None => tags
            .iter()
            .copied()
            .filter(|&t| active != Some(t))
            .max()
            .ok_or_else(|| "no earlier version retained".to_string()),
    }
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

/// What a parallel hand-off leaves alone: the active version's names (none
/// without one) and the candidate's container.
fn parallel_keep(name: &UnitName, active: Option<u32>, candidate: u32) -> Vec<String> {
    let mut keep: Vec<String> = active
        .map(|v| container_names(name, v).to_vec())
        .unwrap_or_default();
    keep.push(name.container_name(candidate));
    keep
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

/// Stop and remove every container of `name` not in `keep`. A stop an
/// earlier deploy missed (interrupted, or podman did not answer) leaves a
/// container that shares the DNS alias with the one about to start, and
/// nothing else revisits it: `--replace` only evicts the same name. Best
/// effort - the next deploy runs it again.
fn evict_strays(name: &UnitName, keep: &[String]) {
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

/// The containers of `name` among `ps_names` (one per line) not in `keep`:
/// the legacy `dpl--{unit}` or any `dpl--{unit}--{version}`. Exact, since the
/// `podman ps` name filter is a substring match that also returns other units
/// sharing the prefix.
fn stray_containers<'a>(name: &UnitName, keep: &[String], ps_names: &'a str) -> Vec<&'a str> {
    let legacy = name.scoped_unit_name();
    let prefix = format!("{legacy}--");
    ps_names
        .lines()
        .map(str::trim)
        .filter(|n| !keep.iter().any(|k| k == n))
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
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn adopt_runtime_log_moves_a_legacy_file_and_keeps_a_live_link() {
        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let name = UnitName::new("web").unwrap();
        let link = ctx.runtime_log_path(&name);
        let log_dir = ctx.unit_log_dir(&name);

        // A unit deployed by an older dpl: a regular file with its ring.
        fs::create_dir_all(&log_dir).unwrap();
        fs::write(&link, "legacy").unwrap();
        fs::write(log_dir.join("runtime.log.1"), "older").unwrap();
        adopt_runtime_log(&ctx, &name, 3);
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("runtime-v3.log")
        );
        assert_eq!(fs::read_to_string(&link).unwrap(), "legacy");
        assert_eq!(
            fs::read_to_string(log_dir.join("runtime-v3.log.1")).unwrap(),
            "older"
        );
        assert!(!log_dir.join("runtime.log.1").exists());

        // Already a live link: the deploy owns where it points.
        adopt_runtime_log(&ctx, &name, 4);
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("runtime-v3.log")
        );

        // Dangling (v3 discarded): re-pointed at the version starting.
        fs::remove_file(log_dir.join("runtime-v3.log")).unwrap();
        adopt_runtime_log(&ctx, &name, 5);
        assert_eq!(
            fs::read_link(&link).unwrap(),
            PathBuf::from("runtime-v5.log")
        );
    }

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
    fn parallel_keep_is_the_active_names_plus_the_candidate() {
        let app = UnitName::new("web").unwrap();
        assert_eq!(
            parallel_keep(&app, Some(1), 3),
            ["dpl--web--1", "dpl--web", "dpl--web--3"]
        );
        assert_eq!(parallel_keep(&app, None, 3), ["dpl--web--3"]);
    }

    #[test]
    fn stray_containers_are_the_units_other_names_only() {
        let app = UnitName::new("web").unwrap();
        // What `podman ps --filter name=dpl--web` returns: every name
        // containing the substring, other units included.
        let ps = "dpl--web--3\ndpl--web--1\ndpl--web\ndpl--web-api\n\
                  dpl--web-api--2\ndpl--webapp--2\ndpl--web--x\n";
        let keep =
            |names: &[&str]| -> Vec<String> { names.iter().map(|n| n.to_string()).collect() };
        assert_eq!(
            stray_containers(&app, &keep(&["dpl--web--3"]), ps),
            ["dpl--web--1", "dpl--web"]
        );
        assert_eq!(
            stray_containers(&app, &keep(&["dpl--web--1", "dpl--web--3"]), ps),
            ["dpl--web"]
        );
        // A parallel hand-off keeps the old version (versioned + legacy) too.
        let both = keep(&["dpl--web--1", "dpl--web", "dpl--web--3"]);
        assert_eq!(stray_containers(&app, &both, ps), Vec::<&str>::new());
        // Nothing kept: everything of the unit, other units untouched.
        assert_eq!(
            stray_containers(&app, &[], ps),
            ["dpl--web--3", "dpl--web--1", "dpl--web"]
        );
    }

    #[test]
    fn numeric_tags_are_the_versions_highest_first() {
        let tags: Vec<String> = ["2", "latest", "10", "", "3", "v4", "+5"]
            .iter()
            .map(|t| t.to_string())
            .collect();
        assert_eq!(numeric_tags(&tags), [10, 3, 2]);
        assert!(numeric_tags(&[]).is_empty());
    }

    #[test]
    fn keep_versions_is_new_plus_the_highest_others() {
        assert_eq!(keep_versions(&[], 5), [5]); // no other tags
        assert_eq!(keep_versions(&[4], 5), [5, 4]); // one other
        assert_eq!(keep_versions(&[5, 4, 3, 2], 5), [5, 4]); // only KEEP highest
        assert_eq!(keep_versions(&[2, 9, 4], 5), [5, 9]); // unsorted; new never dropped
    }

    #[test]
    fn rollback_target_is_a_retained_non_active_version() {
        let tags = [3, 2];
        assert_eq!(rollback_target(&tags, Some(3), None), Ok(2));
        assert_eq!(rollback_target(&tags, None, None), Ok(3));
        assert_eq!(rollback_target(&tags, Some(3), Some(2)), Ok(2));
        assert_eq!(
            rollback_target(&tags, Some(3), Some(3)).unwrap_err(),
            "version 3 is already active"
        );
        assert_eq!(
            rollback_target(&tags, Some(3), Some(7)).unwrap_err(),
            "version 7 is not retained (retained: 3, 2)"
        );
        assert_eq!(
            rollback_target(&[3], Some(3), None).unwrap_err(),
            "no earlier version retained"
        );
        assert_eq!(
            rollback_target(&[], None, None).unwrap_err(),
            "no earlier version retained"
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
