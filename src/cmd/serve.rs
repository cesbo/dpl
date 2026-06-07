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

    // SIGHUP: a deploy handed off a container - reconcile now.
    let reload = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGHUP, Arc::clone(&reload))
        .with_context(|| format!("register {} signal", SIGHUP))?;

    // SIGCHLD: a supervised child died - restart it promptly, not at next poll.
    let child_exit = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGCHLD, Arc::clone(&child_exit))
        .with_context(|| format!("register {} signal", SIGCHLD))?;

    // Timers run on their own thread so a slow timer script can't stall
    // container supervision.
    thread::scope(|scope| {
        scope.spawn(|| timer_loop(ctx, &shutdown));
        supervise(ctx, &shutdown, &reload, &child_exit);
    });

    Ok(())
}

/// Supervise containers: sleep until the next respawn or a signal, then
/// reconcile. The wake cause decides how much work to do (see below).
fn supervise(
    ctx: &MainContext,
    shutdown: &AtomicBool,
    reload: &AtomicBool,
    child_exit: &AtomicBool,
) {
    let mut supervisor = crate::scheduler::Supervisor::new();
    let mut next_wake = supervisor.reconcile(ctx);

    while !shutdown.load(Ordering::Relaxed) {
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

        // Clear before the work: a signal during reconcile re-arms the flag and
        // is serviced next pass, never lost.
        let reloaded = reload.swap(false, Ordering::Relaxed);
        let child_died = child_exit.swap(false, Ordering::Relaxed);

        // A bare SIGCHLD only reaps our children. A full reconcile forks podman,
        // whose own SIGCHLD would re-trigger it into a spin - so only SIGHUP and
        // the timeout take the full path.
        next_wake = if remaining.is_zero() || reloaded {
            supervisor.reconcile(ctx)
        } else if child_died {
            supervisor.reap_exited()
        } else {
            next_wake
        };
    }

    supervisor.shutdown();
}

/// Fire due timers every [`POLL_INTERVAL`] until shutdown.
fn timer_loop(ctx: &MainContext, shutdown: &AtomicBool) {
    while !shutdown.load(Ordering::Relaxed) {
        crate::scheduler::tick(ctx);

        let mut remaining = POLL_INTERVAL;
        while !shutdown.load(Ordering::Relaxed) && !remaining.is_zero() {
            let slice = remaining.min(SLEEP_SLICE);
            thread::sleep(slice);
            remaining = remaining.saturating_sub(slice);
        }
    }
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
