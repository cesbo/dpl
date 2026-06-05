use std::{
    fs,
    fs::{
        File,
        OpenOptions,
    },
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering,
        },
    },
    thread,
    time::Duration,
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

fn tick(ctx: &MainContext) {
    let _ = ctx;
    // TODO: continue here...
}

pub fn run(ctx: &MainContext) -> Result<()> {
    let _lock = SchedulerLock::acquire(ctx)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    for signal in [SIGTERM, SIGINT] {
        signal_hook::flag::register(signal, Arc::clone(&shutdown))
            .with_context(|| format!("register {} signal", signal))?;
    }

    let reload = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(SIGHUP, Arc::clone(&reload))
        .with_context(|| format!("register {} signal", SIGHUP))?;

    tracing::info!("dpl scheduler started");

    tick(ctx);

    while !shutdown.load(Ordering::Relaxed) {
        if reload.swap(false, Ordering::Relaxed) {
            // sighup
        }

        let mut remaining = POLL_INTERVAL;
        while !shutdown.load(Ordering::Relaxed) {
            if remaining > SLEEP_SLICE {
                thread::sleep(SLEEP_SLICE);
                remaining = remaining.saturating_sub(SLEEP_SLICE);
            } else {
                thread::sleep(remaining);
                break;
            }
        }

        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        tick(ctx);
    }

    Ok(())
}

/// Holds the single-instance `flock` for the daemon's lifetime.
struct SchedulerLock {
    #[allow(dead_code)]
    file: File,
    path: PathBuf,
}

impl SchedulerLock {
    fn acquire(ctx: &MainContext) -> Result<Self> {
        let state_dir = ctx.state_dir();
        fs::create_dir_all(&state_dir).context("create state directory")?;
        let path = state_dir.join("scheduler.lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&path)
            .context("create scheduler lock file")?;

        let lock = file
            .try_lock_exclusive()
            .context("acquire scheduler lock")?;

        if !lock {
            bail!("scheduler already started");
        }

        Ok(SchedulerLock { file, path })
    }
}

impl Drop for SchedulerLock {
    fn drop(&mut self) {
        if let Err(err) = std::fs::remove_file(&self.path)
            && err.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("remove scheduler lock '{}': {err}", self.path.display());
        }
    }
}
