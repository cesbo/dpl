pub mod cri_log;
pub mod jsonl_log;

use std::{
    cell::RefCell,
    fmt,
    io::{
        self,
        Write,
    },
    path::Path,
    sync::{
        Arc,
        Mutex,
        MutexGuard,
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

use chrono::{
    DateTime,
    Utc,
};
use indicatif::ProgressBar;

use crate::{
    config::UnitName,
    spinner::Spinner,
};

/// How often a non-TTY open phase line is repeated. Same interval as the
/// readiness heartbeat, so a pipe or CI log does not sit silent through a long
/// build, pull, or extract. A phase that finishes sooner stays one `... ok` line.
const PHASE_HEARTBEAT: Duration = Duration::from_secs(10);

/// Wake cadence of the repeater. Shorter than [`PHASE_HEARTBEAT`] so a line
/// opened just after a wake still repeats on time; dropping the console unparks
/// the thread immediately.
const HEARTBEAT_POLL: Duration = Duration::from_secs(1);

thread_local! {
    static CONSOLE: RefCell<Option<Arc<Mutex<ConsoleState>>>> = const { RefCell::new(None) };
}

/// Live deploy console: the spinner plus the name of the phase in progress.
struct ConsoleState {
    spinner: Spinner,
    started: Instant,
    /// Phase shown on the spinner, echoed as `✓` when the next phase opens.
    phase: Option<String>,
    /// Non-TTY: text of the partial `[MM:SS] <text>...` line awaiting ` ok`.
    open_line: Option<OpenLine>,
}

/// Non-TTY phase line that is still waiting for its `ok`.
struct OpenLine {
    text: String,
    at: Instant,
}

impl OpenLine {
    /// Text to reprint, once the line has been sitting open for [`PHASE_HEARTBEAT`].
    fn repeat_text(&self, now: Instant) -> Option<&str> {
        if now.saturating_duration_since(self.at) >= PHASE_HEARTBEAT {
            Some(self.text.as_str())
        } else {
            None
        }
    }
}

/// Write to stderr and flush for partial (newline-less) lines.
fn write_err(bytes: &[u8]) {
    let mut err = io::stderr().lock();
    let _ = err.write_all(bytes);
    let _ = err.flush();
}

/// Open a non-TTY partial phase line: `[MM:SS] <name>...`.
fn open_phase_line(started: Instant, name: &str) {
    write_err(format!("[{}] {name}...", fmt_stamp(started.elapsed())).as_bytes());
}

fn lock_console(shared: &Mutex<ConsoleState>) -> MutexGuard<'_, ConsoleState> {
    shared.lock().unwrap_or_else(|err| err.into_inner())
}

/// Run `f` against the live console. Returns whether a deploy console is open.
fn with_console(f: impl FnOnce(&mut ConsoleState)) -> bool {
    let Some(shared) = CONSOLE.with(|console| console.borrow().as_ref().cloned()) else {
        return false;
    };
    f(&mut lock_console(&shared));
    true
}

fn set_open_line(state: &mut ConsoleState, text: &str) {
    open_phase_line(state.started, text);
    state.open_line = Some(OpenLine {
        text: text.to_string(),
        at: Instant::now(),
    });
}

/// End the partial line, if one is open. `suffix` is ` ok\n` when the phase
/// finished and `\n` when it is interrupted or repeated.
fn close_open_line(state: &mut ConsoleState, suffix: &[u8]) {
    if state.open_line.take().is_some() {
        write_err(suffix);
    }
}

/// Reprint a non-TTY line that has been open for [`PHASE_HEARTBEAT`], with a
/// fresh stamp. The pending `ok` still lands on the latest copy.
fn repeat_open_line(state: &mut ConsoleState) {
    let now = Instant::now();
    let Some(text) = state
        .open_line
        .as_ref()
        .and_then(|line| line.repeat_text(now))
        .map(str::to_string)
    else {
        return;
    };
    close_open_line(state, b"\n");
    set_open_line(state, &text);
}

fn spawn_heartbeat(
    shared: Arc<Mutex<ConsoleState>>,
    stop: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("dpl-heartbeat".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                thread::park_timeout(HEARTBEAT_POLL);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                repeat_open_line(&mut lock_console(&shared));
            }
        })
        .expect("spawn deploy heartbeat")
}

/// Stops the non-TTY repeater. Joined on drop so a late tick cannot land after
/// the console is gone.
struct Heartbeat {
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            handle.thread().unpark();
            let _ = handle.join();
        }
    }
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
    with_console(|state| {
        let message = message.to_string();
        if state.spinner.is_tty() {
            state.spinner.bar().set_message(message.clone());
            if let Some(prev) = state.phase.replace(message) {
                echo_phase_done(state.spinner.bar(), state.started, &prev);
            }
        } else {
            close_open_line(state, b" ok\n");
            set_open_line(state, &message);
            state.phase = Some(message);
        }
    });
}

/// Refresh the phase in progress with live detail (what is being waited for,
/// elapsed vs budget). On a TTY it replaces the spinner message; off it the
/// stamped progress line takes over as the phase's open line, so the pending
/// `ok` still lands - after the last progress line instead of the first. A
/// refresh also resets the non-TTY repeat timer. No-op outside a deploy.
pub fn progress(message: impl fmt::Display) {
    with_console(|state| {
        let message = message.to_string();
        if state.spinner.is_tty() {
            state.spinner.bar().set_message(message);
            return;
        }

        close_open_line(state, b"\n");
        set_open_line(state, &message);
    });
}

/// Drives the deploy spinner and the `[MM:SS] ✓ <phase>` lines.
///
/// Off a TTY, an open phase line is repeated every 10s so a long build or pull
/// does not look stuck in a pipe or CI log.
pub struct DeployConsole {
    heartbeat: Option<Heartbeat>,
}

impl DeployConsole {
    pub fn open(unit: &UnitName, version: u32) -> Self {
        let spinner = Spinner::with_style(
            format!("{unit} v{version}: starting"),
            crate::spinner::deploy_style(),
        );
        let is_tty = spinner.is_tty();
        let shared = Arc::new(Mutex::new(ConsoleState {
            spinner,
            started: Instant::now(),
            phase: None,
            open_line: None,
        }));
        CONSOLE.with_borrow_mut(|console| {
            *console = Some(Arc::clone(&shared));
        });
        let heartbeat = if is_tty {
            None
        } else {
            let stop = Arc::new(AtomicBool::new(false));
            let handle = spawn_heartbeat(Arc::clone(&shared), Arc::clone(&stop));
            Some(Heartbeat {
                stop,
                handle: Some(handle),
            })
        };
        DeployConsole { heartbeat }
    }

    /// Stop the spinner and print the success summary.
    pub fn finish_ok(&self) {
        with_console(|state| {
            if state.spinner.is_tty() {
                if let Some(prev) = state.phase.take() {
                    echo_phase_done(state.spinner.bar(), state.started, &prev);
                }
            } else {
                close_open_line(state, b" ok\n");
                state.phase = None;
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
        with_console(|state| {
            close_open_line(state, b"\n");
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
        // Join the repeater before clearing the console it writes through.
        drop(self.heartbeat.take());
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
    let message = message.to_string();
    let printed = with_console(|state| {
        // Keep the phase visible after the warning so a long build still repeats.
        let resume = state.open_line.as_ref().map(|line| line.text.clone());
        close_open_line(state, b"\n");
        let line = format!(
            "[{}] {deploy_tag}: {message}",
            fmt_stamp(state.started.elapsed())
        );
        crate::spinner::print_above(state.spinner.bar(), line.as_bytes());
        if !state.spinner.is_tty()
            && let Some(text) = resume
        {
            set_open_line(state, &text);
        }
    });
    if !printed {
        eprintln!("{plain_tag}: {message}");
    }
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
    fn open_line_repeats_after_heartbeat_not_before() {
        let at = Instant::now();
        let line = OpenLine {
            text: "building app image".into(),
            at,
        };

        assert_eq!(line.repeat_text(at), None);
        assert_eq!(
            line.repeat_text(at + PHASE_HEARTBEAT - Duration::from_millis(1)),
            None
        );
        assert_eq!(
            line.repeat_text(at + PHASE_HEARTBEAT),
            Some("building app image")
        );
    }

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
