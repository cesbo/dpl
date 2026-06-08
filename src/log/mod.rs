pub mod cri_log;

use std::{
    cell::RefCell,
    fmt,
    path::Path,
    time::{
        Duration,
        Instant,
    },
};

use chrono::{
    DateTime,
    Utc,
};
use indicatif::ProgressBar;

use crate::{
    config::UnitName,
    spinner::Spinner,
};

thread_local! {
    static CONSOLE: RefCell<Option<ConsoleState>> = const { RefCell::new(None) };
}

/// Live deploy console: the spinner plus the name of the phase in progress.
struct ConsoleState {
    spinner: Spinner,
    started: Instant,
    /// Phase shown on the spinner, echoed as `✓` when the next phase opens.
    phase: Option<String>,
}

pub fn success_mark() -> console::StyledObject<&'static str> {
    console::style("✓").green()
}

pub fn error_mark() -> console::StyledObject<&'static str> {
    console::style("✗").red()
}

/// Print an aligned `Key:    value` line to stdout for `dpl inspect` reports.
/// The key (plus its colon) is left-padded to a fixed column so values line up.
pub fn print_field(key: &str, value: impl fmt::Display) {
    let key = format!("{key}:");
    println!("{key:<20} {value}")
}

/// Advance the deploy console to a new phase: show it on the spinner and leave a
/// `✓` for the phase that just finished. No-op outside a deploy.
pub fn phase(message: impl fmt::Display) {
    CONSOLE.with_borrow_mut(|console| {
        let Some(state) = console.as_mut() else {
            return;
        };
        let message = message.to_string();
        state.spinner.bar().set_message(message.clone());
        if let Some(prev) = state.phase.replace(message) {
            echo_phase_done(state.spinner.bar(), state.started, &prev);
        }
    });
}

/// Drives the deploy spinner and the `[MM:SS] ✓ <phase>` lines.
pub struct DeployConsole;

impl DeployConsole {
    pub fn open(unit: &UnitName, version: u32) -> Self {
        let spinner = Spinner::with_style(
            format!("{unit} v{version}: starting"),
            crate::spinner::deploy_style(),
        );
        CONSOLE.with_borrow_mut(|console| {
            *console = Some(ConsoleState {
                spinner,
                started: Instant::now(),
                phase: None,
            });
        });
        DeployConsole
    }

    /// Stop the spinner and print the success summary.
    pub fn finish_ok(&self) {
        CONSOLE.with_borrow_mut(|console| {
            let Some(state) = console.as_mut() else {
                return;
            };
            if let Some(prev) = state.phase.take() {
                echo_phase_done(state.spinner.bar(), state.started, &prev);
            }
            state.spinner.finish();
            eprintln!(
                "[{}] {}  deployed",
                fmt_stamp(state.started.elapsed()),
                success_mark()
            );
        });
    }

    /// Stop the spinner and print the failure summary.
    pub fn finish_err(&self, cause: &str, log_path: &Path) {
        CONSOLE.with_borrow_mut(|console| {
            let Some(state) = console.as_mut() else {
                return;
            };
            state.phase.take();
            let stamp = fmt_stamp(state.started.elapsed());
            state.spinner.finish();
            let detail = if cause.is_empty() {
                String::new()
            } else {
                format!(": {cause}")
            };
            if log_path.exists() {
                eprintln!("[{stamp}] {}  failed{detail}", error_mark());
                eprintln!("{}Log: {}", " ".repeat(stamp.len() + 6), log_path.display());
            } else {
                eprintln!("[{stamp}] {}  failed{detail}", error_mark());
            }
        });
    }
}

impl Drop for DeployConsole {
    fn drop(&mut self) {
        CONSOLE.with_borrow_mut(|console| *console = None);
    }
}

/// Print a warning above the spinner.
pub fn warn(message: impl fmt::Display) {
    emit("WARN", "warning", message);
}

/// Print a error above the spinner.
pub fn error(message: impl fmt::Display) {
    emit("ERROR", "error", message);
}

fn emit(deploy_tag: &str, plain_tag: &str, message: impl fmt::Display) {
    CONSOLE.with_borrow(|console| match console.as_ref() {
        Some(state) => {
            let line = format!(
                "[{}] {deploy_tag}: {message}",
                fmt_stamp(state.started.elapsed())
            );
            crate::spinner::print_above(state.spinner.bar(), line.as_bytes());
        }
        None => eprintln!("{plain_tag}: {message}"),
    });
}

/// Print a `[mm:ss] ✓ <name>` line for a completed phase above the spinner.
fn echo_phase_done(bar: &ProgressBar, started: Instant, name: &str) {
    let line = format!(
        "[{}] {}  {name}",
        fmt_stamp(started.elapsed()),
        success_mark()
    );
    crate::spinner::print_above(bar, line.as_bytes());
}

pub fn fmt_stamp(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// Human-readable duration
pub fn fmt_duration(duration: Duration) -> String {
    let s = duration.as_secs();
    match s {
        0 .. 100 => format!("{s}s"),
        _ => format!("{}m", (s + 30) / 60),
    }
}

/// How long ago `then` was relative to `now`: "just now", "2 min ago",
/// "3 hours ago", "5 days ago".
pub fn fmt_ago(now: &DateTime<Utc>, then: &DateTime<Utc>) -> String {
    let s = now.signed_duration_since(then).num_seconds();
    match s {
        0 .. 10 => "now".to_string(),
        10 .. 100 => format!("{s}s ago"),
        100 .. 3600 => format!("{}m ago", s / 60),
        3600 .. 86400 => format!("{}h ago", s / 3600),
        _ => {
            if s > 0 {
                format!("{}d ago", s / 86400)
            } else {
                "?".into()
            }
        }
    }
}

/// Human-readable byte size in decimal (SI) units - `734 B`, `12.0 kB`, `21.5 MB`.
/// Matching podman decimal formatting in `stats`/`images`.
pub fn fmt_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "kB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1000.0 && unit < UNITS.len() - 1 {
        size /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_stamp_under_hour() {
        assert_eq!(fmt_stamp(Duration::from_secs(0)), "00:00");
        assert_eq!(fmt_stamp(Duration::from_secs(83)), "01:23");
        assert_eq!(fmt_stamp(Duration::from_secs(3599)), "59:59");
    }

    #[test]
    fn fmt_stamp_with_hours() {
        assert_eq!(fmt_stamp(Duration::from_secs(3600)), "01:00:00");
        assert_eq!(fmt_stamp(Duration::from_secs(3725)), "01:02:05");
    }

    #[test]
    fn fmt_ago_buckets() {
        let now = DateTime::parse_from_rfc3339("2026-05-31T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ago = |secs: i64| fmt_ago(&now, &(now - chrono::Duration::seconds(secs)));

        assert_eq!(ago(0), "now");
        assert_eq!(ago(9), "now");
        assert_eq!(ago(30), "30s ago");
        assert_eq!(ago(90), "90s ago");
        assert_eq!(ago(120), "2m ago");
        assert_eq!(ago(3600), "1h ago");
        assert_eq!(ago(7200), "2h ago");
        assert_eq!(ago(86400), "1d ago");
        assert_eq!(ago(3 * 86400), "3d ago");
    }

    #[test]
    fn fmt_bytes_scales_by_decimal_units() {
        assert_eq!(fmt_bytes(0), "0 B");
        assert_eq!(fmt_bytes(734), "734 B");
        assert_eq!(fmt_bytes(999), "999 B");
        assert_eq!(fmt_bytes(1_000), "1.0 kB");
        assert_eq!(fmt_bytes(4_718_592), "4.7 MB");
        assert_eq!(fmt_bytes(210_542_080), "210.5 MB");
        assert_eq!(fmt_bytes(4_000_000_000), "4.0 GB");
    }
}
