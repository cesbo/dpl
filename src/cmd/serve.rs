use std::{
    fs,
    fs::{
        File,
        OpenOptions,
    },
    io,
    os::unix::fs::FileExt as UnixFileExt,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
    thread,
    time::{
        Duration,
        Instant,
    },
};

use anyhow::{
    Context,
    Result,
    bail,
};
use fs4::fs_std::FileExt;
use signal_hook::consts::{
    SIGCHLD,
    SIGHUP,
    SIGINT,
    SIGTERM,
};

use crate::MainContext;

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const SLEEP_SLICE: Duration = Duration::from_millis(250);

pub fn run(ctx: &MainContext) -> Result<()> {
    let _lock = SchedulerLock::acquire(ctx)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&shutdown))
            .with_context(|| format!("register {} signal", signal))?;
    }

    // SIGHUP wakes the loop early to reconcile now (a deploy sends it after
    // handing a container off to the supervisor).
    let reload = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGHUP, Arc::clone(&reload))
        .with_context(|| format!("register {} signal", SIGHUP))?;

    // SIGCHLD wakes the loop the instant a supervised child dies, so a crashed
    // container is noticed immediately instead of at the next poll.
    let child_exit = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGCHLD, Arc::clone(&child_exit))
        .with_context(|| format!("register {} signal", SIGCHLD))?;

    let mut supervisor = crate::scheduler::Supervisor::new();

    crate::scheduler::tick(ctx);
    let mut next_wake = supervisor.reconcile(ctx);

    while !shutdown.load(Ordering::Relaxed) {
        // Sleep until the next pending respawn.
        let mut remaining = next_wake
            .map(|at| at.saturating_duration_since(Instant::now()))
            .unwrap_or(POLL_INTERVAL)
            .min(POLL_INTERVAL);

        while !shutdown.load(Ordering::Relaxed)
            && !reload.load(Ordering::Relaxed)
            && !child_exit.load(Ordering::Relaxed)
            && !remaining.is_zero()
        {
            let slice = remaining.min(SLEEP_SLICE);
            thread::sleep(slice);
            remaining = remaining.saturating_sub(slice);
        }

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        reload.store(false, Ordering::Relaxed);
        child_exit.store(false, Ordering::Relaxed);
        crate::scheduler::tick(ctx);
        next_wake = supervisor.reconcile(ctx);
    }

    supervisor.shutdown();

    Ok(())
}

/// Single-instance `flock` and pidfile in one file, held for the daemon's
/// lifetime. The exclusive lock enforces one `dpl serve`; the PID written into
/// the same file lets a deploy find the daemon and SIGHUP it (see [`notify`]).
struct SchedulerLock {
    // Held for the daemon's lifetime: closing the fd releases the flock.
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl SchedulerLock {
    fn acquire(ctx: &MainContext) -> Result<Self> {
        fs::create_dir_all(ctx.state_dir()).context("create state directory")?;
        let path = ctx.scheduler_pid_path();
        // Do NOT truncate on open: a losing second instance must not wipe the
        // winner's PID before its lock attempt fails. We rewrite the PID only
        // after the lock is ours.
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .context("create scheduler pidfile")?;

        let lock = file
            .try_lock_exclusive()
            .context("acquire scheduler lock")?;

        if !lock {
            bail!("scheduler already started");
        }

        // Lock is ours: rewrite in place (write then truncate to length). In
        // place, not temp+rename - a rename would swap the inode and orphan the
        // flock.
        let pid = std::process::id().to_string();
        file.write_all_at(pid.as_bytes(), 0)
            .context("write scheduler pidfile")?;
        file.set_len(pid.len() as u64)
            .context("truncate scheduler pidfile")?;

        Ok(SchedulerLock { file, path })
    }
}

impl Drop for SchedulerLock {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            crate::log::warn(format!(
                "remove scheduler pidfile '{}': {err}",
                self.path.display()
            ));
        }
    }
}
