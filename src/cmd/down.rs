use std::{
    process::Command,
    thread,
    time::{
        Duration,
        Instant,
    },
};

use anyhow::{
    Context,
    Result,
};

use crate::{
    MainContext,
    log::success_mark,
    podman,
    state::DeployState,
};

/// How long to wait for `dpl serve` to drop its pidfile after SIGTERM before we
/// stop containers anyway.
const SERVE_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_SLICE: Duration = Duration::from_millis(100);

/// Tear down the whole host: stop and remove every supervised container.
///
/// Deploy state is left intact, so restarting `dpl serve` brings
/// everything back.
pub fn run(ctx: &MainContext) -> Result<()> {
    // Stop serve first so it can't respawn a container we are about to stop.
    stop_serve(ctx)?;

    // Same source as the supervisor's desired set: deploy state, not configs.
    let supervised = supervised_targets(ctx);

    if supervised.is_empty() {
        eprintln!("{} nothing to tear down", success_mark());
        return Ok(());
    }

    let mut stopped = 0usize;
    for name in &supervised {
        match podman::stop_and_remove(name) {
            Ok(()) => stopped += 1,
            Err(err) => crate::log::warn(format!("stop container '{}': {err}", name.as_str())),
        }
    }

    eprintln!("{} stopped {stopped} container(s)", success_mark());
    Ok(())
}

fn supervised_targets(ctx: &MainContext) -> Vec<crate::config::UnitName> {
    DeployState::list(ctx)
        .into_iter()
        .filter(|(_, state)| state.supervised)
        .map(|(name, _)| name)
        .collect()
}

fn stop_serve(ctx: &MainContext) -> Result<()> {
    let path = ctx.serve_pid_path();
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let Ok(pid) = content.trim().parse::<u32>() else {
        return Ok(());
    };

    // A stale pidfile (no live process) is harmless: nothing to stop.
    if !process_alive(pid) {
        return Ok(());
    }

    Command::new("kill")
        .arg(pid.to_string())
        .status()
        .with_context(|| format!("signal serve pid {pid}"))?;

    let deadline = Instant::now() + SERVE_STOP_TIMEOUT;
    while Instant::now() < deadline {
        if !path.exists() || !process_alive(pid) {
            return Ok(());
        }
        thread::sleep(POLL_SLICE);
    }

    crate::log::warn(format!(
        "serve did not stop within {}s; forcing termination",
        SERVE_STOP_TIMEOUT.as_secs()
    ));

    Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status()
        .with_context(|| format!("kill serve pid {pid}"))?;

    Ok(())
}

fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::config::UnitName;

    fn ctx() -> (TempDir, MainContext) {
        let dir = TempDir::new().unwrap();
        let ctx = MainContext {
            base: dir.path().to_path_buf(),
            master_key: None,
        };
        (dir, ctx)
    }

    /// Hand off through the serve hand-off (`set_check`), so the state file
    /// carries the `supervised` flag - what `dpl start`/the supervisor see.
    fn deploy_supervised(ctx: &MainContext, name: &str, kind: &str) {
        let name = UnitName::new(name).unwrap();
        let (_guard, mut state) = DeployState::acquire(ctx, &name).unwrap();
        state.begin_deploy(kind).unwrap();
        state.set_check();
    }

    /// Deploy without a serve hand-off (`set_ready` only), as db/domain do - no
    /// `supervised` flag, no container.
    fn deploy_plain(ctx: &MainContext, name: &str) {
        let name = UnitName::new(name).unwrap();
        let (_guard, mut state) = DeployState::acquire(ctx, &name).unwrap();
        state.begin_deploy("domain").unwrap();
        state.set_ready();
    }

    #[test]
    fn targets_only_the_supervised_set() {
        let (_dir, ctx) = ctx();

        deploy_supervised(&ctx, "web", "app");
        deploy_supervised(&ctx, "pg", "db-server");
        deploy_plain(&ctx, "dbx");
        deploy_plain(&ctx, "site-com");
        // Configured but never deployed: no state file, so not a target.
        ctx.write_test_unit("app-static", "type: app\nimage: alpine\nbuilds: []\n");

        let mut targets = supervised_targets(&ctx);
        targets.sort();
        assert_eq!(
            targets,
            vec![UnitName::new("pg").unwrap(), UnitName::new("web").unwrap()]
        );
    }

    #[test]
    fn no_supervised_units_yields_no_targets() {
        let (_dir, ctx) = ctx();
        deploy_plain(&ctx, "site-com");
        assert!(supervised_targets(&ctx).is_empty());
    }
}
