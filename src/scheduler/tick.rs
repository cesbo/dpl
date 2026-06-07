use chrono::{
    DateTime,
    Utc,
};

use crate::{
    MainContext,
    log,
    scheduler::run_timer,
    state::{
        DeployLockGuard,
        DeployState,
    },
    timers::TimersState,
};

pub fn tick(ctx: &MainContext) {
    let now = Utc::now();

    for (name, state) in DeployState::list(ctx) {
        if !state.supervised {
            continue;
        }

        if !ctx.timers_state_path(&name).exists() {
            continue;
        }

        let (_timer_lock, mut timers) = match TimersState::acquire(ctx, &name) {
            Ok(acquired) => acquired,
            Err(err) => {
                log::warn(format!("scheduler: acquire timers for '{name}': {err}"));
                continue;
            }
        };

        let due = due_timers(&timers, now);
        if due.is_empty() {
            continue;
        }

        match DeployLockGuard::try_acquire(ctx, &name) {
            Ok(Some(_deploy_guard)) => {
                for timer_name in due {
                    if let Err(err) = run_timer(ctx, &name, &mut timers, &timer_name) {
                        log::warn(format!(
                            "scheduler: timer '{timer_name}' on '{name}': {err}"
                        ));
                    }
                }
            }
            // Busy with a deploy; these timers run on a later tick.
            Ok(None) => {}
            Err(err) => log::warn(format!("scheduler: deploy lock '{name}': {err}")),
        }
    }
}

/// Names of timers whose next run is at or before `now`.
fn due_timers(timers: &TimersState, now: DateTime<Utc>) -> Vec<String> {
    timers
        .timers
        .iter()
        .filter(|(_, state)| state.next_run.is_some_and(|next| next <= now))
        .map(|(name, _)| name.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use croner::Cron;

    use super::*;
    use crate::timers::TimerState;

    fn cron(expr: &str) -> Cron {
        expr.parse().unwrap()
    }

    fn state_with(entries: Vec<(&str, Option<DateTime<Utc>>)>) -> TimersState {
        let anchor = DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let mut timers = BTreeMap::new();
        for (name, next_run) in entries {
            timers.insert(
                name.to_string(),
                TimerState::idle(cron("0 * * * *"), anchor, next_run),
            );
        }
        // Field is private; deserialize a transparent map to build the state.
        serde_json::from_value(serde_json::to_value(&timers).unwrap()).unwrap()
    }

    #[test]
    fn due_selects_past_and_now_excludes_future_and_none() {
        let now = DateTime::<Utc>::from_timestamp(1_700_001_000, 0).unwrap();
        let past = now - chrono::Duration::seconds(60);
        let future = now + chrono::Duration::seconds(60);

        let timers = state_with(vec![
            ("past", Some(past)),
            ("now", Some(now)),
            ("future", Some(future)),
            ("parked", None),
        ]);

        let mut due = due_timers(&timers, now);
        due.sort();
        assert_eq!(due, ["now", "past"]);
    }

    #[test]
    fn empty_state_has_nothing_due() {
        let now = DateTime::<Utc>::from_timestamp(1_700_001_000, 0).unwrap();
        assert!(due_timers(&state_with(vec![]), now).is_empty());
    }
}
