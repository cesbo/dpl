use std::{
    io,
    process::{
        Command,
        Stdio,
    },
    time::{
        Duration,
        Instant,
    },
};

use chrono::Utc;
use tracing::{
    info,
    warn,
};

use crate::{
    MainContext,
    config::UnitName,
    scheduler::cri_log::CriLog,
    timers::{
        TimerError,
        TimerState,
        TimersState,
    },
};

/// Run one of a unit's timers once, recording the outcome into `timers`. The
/// container's stdout/stderr are captured to the unit's timer log in CRI
/// `k8s-file` format.
pub fn run_timer(
    ctx: &MainContext,
    name: &UnitName,
    timers: &mut TimersState,
    timer_name: &str,
) -> Result<(), TimerError> {
    // TimersState is the source of truth for which timers exist.
    let prev = timers
        .timers
        .get(timer_name)
        .cloned()
        .ok_or_else(|| TimerError::Unknown(timer_name.to_string()))?;

    let started_at = Utc::now();

    let next_run = TimerState::next_occurrence(&prev.schedule, started_at);
    if next_run.is_none() {
        warn!("timer '{timer_name}': next run unresolved");
    }

    let running = prev.running(started_at, next_run);

    if !crate::podman::is_running(name) {
        let error = format!("container for '{name}' not running");
        info!("{error}; skipping timer '{timer_name}'");
        timers.set_timer_state(timer_name, running.failed(Duration::default(), error));
        return Ok(());
    }

    let container = name.scoped_unit_name();
    let command = format!("timer--{timer_name}");

    timers.set_timer_state(timer_name, running.clone());

    let clock = Instant::now();
    let spawned = Command::new("podman")
        .args(["exec", &container, "/bin/sh", "/opt/dpl/run.sh", &command])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match spawned {
        Ok(child) => child,
        Err(err) => {
            let err = crate::podman::podman_spawn_error(err);
            timers.set_timer_state(timer_name, running.failed(clock.elapsed(), err.to_string()));
            return Err(TimerError::Run {
                name: timer_name.to_string(),
                source: err,
            });
        }
    };

    // Drain the container's output into the unit's timer log before reaping it,
    // so a full pipe can't wedge the process. A log failure is non-fatal: the
    // timer still ran, so record its real outcome rather than masking it.
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let log_path = ctx.timer_log_path(name, timer_name);
    if let Err(err) = CriLog::open(&log_path).and_then(|log| log.capture(stdout, stderr)) {
        warn!("timer '{timer_name}': write log {}: {err}", log_path.display());
    }

    let status = child.wait();
    let elapsed = clock.elapsed();

    match status {
        Ok(status) if status.success() => {
            timers.set_timer_state(timer_name, running.success(elapsed));
            Ok(())
        }
        Ok(status) => {
            let cause = format!("timer script exited with {status}");
            timers.set_timer_state(timer_name, running.failed(elapsed, &cause));
            Err(TimerError::Run {
                name: timer_name.to_string(),
                source: io::Error::other(cause),
            })
        }
        Err(err) => {
            timers.set_timer_state(timer_name, running.failed(elapsed, err.to_string()));
            Err(TimerError::Run {
                name: timer_name.to_string(),
                source: err,
            })
        }
    }
}
