use std::{
    fs,
    fs::{
        File,
        OpenOptions,
    },
    io::{
        self,
        BufRead,
        BufReader,
        Seek,
        SeekFrom,
        Write,
    },
    path::PathBuf,
    process::{
        Command,
        Stdio,
    },
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
    SIGHUP,
    SIGINT,
    SIGTERM,
};

use crate::MainContext;

const POLL_INTERVAL: Duration = Duration::from_secs(10);
const SLEEP_SLICE: Duration = Duration::from_millis(250);

pub fn run(ctx: &MainContext) -> Result<()> {
    let _lock = ServeLock::acquire(ctx)?;

    // Make sure the master key is ready.
    super::secret::load_or_create_key(ctx).context("ensure master key")?;

    eprintln!(
        "dpl serve {} base={}",
        env!("CARGO_PKG_VERSION"),
        ctx.base().display()
    );

    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&shutdown))
            .with_context(|| format!("register {} signal", signal))?;
    }

    // SIGHUP: a deploy handed off a container - reconcile now.
    let reload = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGHUP, Arc::clone(&reload))
        .with_context(|| format!("register {} signal", SIGHUP))?;

    // A supervised container died - reconcile promptly, not at next poll. Set by
    // the podman events watcher; the 10s poll + is_running is the backstop for
    // any death the event stream missed.
    let died = Arc::new(AtomicBool::new(false));

    // Timers and the podman events watcher run on their own threads so neither a
    // slow timer script nor a blocking event read can stall supervision.
    thread::scope(|scope| {
        scope.spawn(|| timer_loop(ctx, &shutdown));
        scope.spawn(|| events_watcher(&shutdown, &died));
        supervise(ctx, &shutdown, &reload, &died);
    });

    Ok(())
}

/// Supervise containers: sleep until the next respawn or a signal, then
/// reconcile. The wake cause decides how much work to do (see below).
fn supervise(ctx: &MainContext, shutdown: &AtomicBool, reload: &AtomicBool, died: &AtomicBool) {
    let mut supervisor = crate::serve::Supervisor::new();
    let mut next_wake = supervisor.reconcile(ctx);

    while !shutdown.load(Ordering::Relaxed) {
        let mut remaining = next_wake
            .map(|at| at.saturating_duration_since(Instant::now()))
            .unwrap_or(POLL_INTERVAL)
            .min(POLL_INTERVAL);

        while !shutdown.load(Ordering::Relaxed)
            && !reload.load(Ordering::Relaxed)
            && !died.load(Ordering::Relaxed)
            && !remaining.is_zero()
        {
            let slice = remaining.min(SLEEP_SLICE);
            thread::sleep(slice);
            remaining = remaining.saturating_sub(slice);
        }

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        // Clear before the work: a signal/event during reconcile re-arms the
        // flag and is serviced next pass, never lost. Liveness comes from
        // podman, so every wake takes the same full reconcile path.
        reload.swap(false, Ordering::Relaxed);
        died.swap(false, Ordering::Relaxed);

        next_wake = supervisor.reconcile(ctx);
    }

    supervisor.shutdown();
}

/// Stream `podman events` and flip `died` on each container death to trigger a
/// prompt reconcile; the 10s poll stays the source-of-truth backstop, this only
/// narrows latency. Restarts the stream if podman drops it; exits on shutdown.
///
/// The `lines()` read blocks, so a watchdog thread kills the child to unblock
/// it. The watchdog wakes on shutdown or when the read loop ends (`done` flag),
/// so a dropped stream lets the scope return and the outer loop re-spawns.
fn events_watcher(shutdown: &AtomicBool, died: &AtomicBool) {
    while !shutdown.load(Ordering::Relaxed) {
        match Command::new("podman")
            .args(["events", "--filter", "event=die", "--format", "{{.Status}}"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                // Take stdout so the watchdog can own the child for kill+wait
                // while the read loop reads the pipe independently.
                let stdout = child.stdout.take();
                // Set when the read loop ends so the watchdog reaps the child
                // even without shutdown.
                let done = AtomicBool::new(false);
                thread::scope(|s| {
                    // Sole owner of kill+wait: reaped exactly once, so no
                    // kill-by-pid can hit a reused PID.
                    s.spawn(|| {
                        while !shutdown.load(Ordering::Relaxed) && !done.load(Ordering::Relaxed) {
                            thread::sleep(SLEEP_SLICE);
                        }
                        let _ = child.kill();
                        let _ = child.wait();
                    });

                    if let Some(out) = stdout {
                        for line in BufReader::new(out).lines() {
                            if line.is_err() || shutdown.load(Ordering::Relaxed) {
                                break;
                            }
                            died.store(true, Ordering::Relaxed);
                        }
                    }
                    // Read ended (EOF, error, or shutdown): release the watchdog.
                    done.store(true, Ordering::Relaxed);
                });
            }
            Err(err) => {
                crate::log::warn(format!("serve: podman events watcher: {err}"));
            }
        }

        // Avoid a hot respawn loop if podman events keeps failing.
        let mut remaining = POLL_INTERVAL;
        while !shutdown.load(Ordering::Relaxed) && !remaining.is_zero() {
            let slice = remaining.min(SLEEP_SLICE);
            thread::sleep(slice);
            remaining = remaining.saturating_sub(slice);
        }
    }
}

/// Fire due timers every [`POLL_INTERVAL`] until shutdown.
fn timer_loop(ctx: &MainContext, shutdown: &AtomicBool) {
    while !shutdown.load(Ordering::Relaxed) {
        crate::serve::tick(ctx);

        let mut remaining = POLL_INTERVAL;
        while !shutdown.load(Ordering::Relaxed) && !remaining.is_zero() {
            let slice = remaining.min(SLEEP_SLICE);
            thread::sleep(slice);
            remaining = remaining.saturating_sub(slice);
        }
    }
}

/// Single-instance `flock` and pidfile in one file, held for the serve process's
/// lifetime. The exclusive lock enforces one `dpl serve`; the PID written into
/// the same file lets a deploy find `dpl serve` and SIGHUP it (see [`notify`]).
struct ServeLock {
    // Held for the serve process's lifetime: closing the fd releases the flock.
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl ServeLock {
    fn acquire(ctx: &MainContext) -> Result<Self> {
        fs::create_dir_all(ctx.state_dir()).context("create state directory")?;
        let path = ctx.serve_pid_path();
        // Do NOT truncate on open: a losing second instance must not wipe the
        // winner's PID before its lock attempt fails. We rewrite the PID only
        // after the lock is ours.
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .context("create serve pidfile")?;

        let lock = file.try_lock_exclusive().context("acquire serve lock")?;

        if !lock {
            bail!("serve already started");
        }

        // Lock is ours: rewrite in place (write then truncate to length). In
        // place, not temp+rename - a rename would swap the inode and orphan the
        // flock.
        let pid = std::process::id().to_string();
        let mut file = file;
        file.seek(SeekFrom::Start(0))
            .context("seek serve pidfile")?;
        file.write_all(pid.as_bytes())
            .context("write serve pidfile")?;
        file.set_len(pid.len() as u64)
            .context("truncate serve pidfile")?;

        Ok(ServeLock { file, path })
    }
}

impl Drop for ServeLock {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            crate::log::warn(format!(
                "remove serve pidfile '{}': {err}",
                self.path.display()
            ));
        }
    }
}
