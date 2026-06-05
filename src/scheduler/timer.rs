use std::{
    io,
    process::Command,
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
    config::UnitName,
    timers::{
        TimerError,
        TimerState,
        TimersState,
    },
};

/// Run one of a unit's timers once, recording the outcome into `timers`.
pub fn run_timer(
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
    let status = Command::new("podman")
        .args(["exec", &container, "/bin/sh", "/opt/dpl/run.sh", &command])
        .status();
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
            let err = crate::podman::podman_spawn_error(err);
            timers.set_timer_state(timer_name, running.failed(elapsed, err.to_string()));
            Err(TimerError::Run {
                name: timer_name.to_string(),
                source: err,
            })
        }
    }
}
