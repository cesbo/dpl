mod supervisor;
mod tick;
mod timer;

use std::process::Command;

pub use self::{
    supervisor::Supervisor,
    tick::tick,
    timer::run_timer,
};
use crate::MainContext;

/// SIGHUP the running `dpl serve` so it reconciles now instead of at its next
/// poll. `false` when there is nothing to nudge - the caller is about to wait
/// for a container that nobody will start, and must say so.
pub fn notify(ctx: &MainContext) -> bool {
    let Ok(content) = std::fs::read_to_string(ctx.serve_pid_path()) else {
        return false;
    };
    let Ok(pid) = content.trim().parse::<u32>() else {
        return false;
    };
    // `kill 0` signals our entire process group, not one process: a corrupted
    // pidfile must never turn a deploy nudge into a self-inflicted SIGHUP.
    if pid == 0 {
        return false;
    }

    Command::new("kill")
        .arg("-HUP")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}

/// Warn when nothing is listening for the hand-off, so the readiness wait that
/// follows is not the first hint that `dpl serve` is down.
pub fn notify_or_warn(ctx: &MainContext, container: &str) {
    if !notify(ctx) {
        crate::log::warn(format!(
            "dpl serve is not running - nothing will start container {container}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn ctx() -> (TempDir, MainContext) {
        let dir = TempDir::new().unwrap();
        let ctx = MainContext {
            base: dir.path().to_path_buf(),
            master_key: None,
        };
        (dir, ctx)
    }

    #[test]
    fn notify_is_false_without_a_pidfile() {
        let (_dir, ctx) = ctx();
        assert!(!notify(&ctx));
    }

    #[test]
    fn notify_is_false_on_an_unparseable_pidfile() {
        let (_dir, ctx) = ctx();
        let path = ctx.serve_pid_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not-a-pid\n").unwrap();
        assert!(!notify(&ctx));
    }

    #[test]
    fn notify_refuses_pid_zero() {
        // `kill 0` would signal our own process group; a corrupted pidfile must
        // not be able to do that.
        let (_dir, ctx) = ctx();
        let path = ctx.serve_pid_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "0\n").unwrap();
        assert!(!notify(&ctx));
    }
}
