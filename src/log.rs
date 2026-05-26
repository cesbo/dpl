use std::{
    fmt::{
        self,
        Write as _,
    },
    fs::OpenOptions,
    io::{
        self,
        BufWriter,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
    sync::Mutex,
    time::{
        Duration,
        Instant,
    },
};

use indicatif::ProgressBar;
use tracing::{
    Dispatch,
    Event,
    Level,
    Subscriber,
    field::{
        Field,
        Visit,
    },
};
use tracing_subscriber::{
    Registry,
    layer::{
        Context,
        Layer,
        SubscriberExt,
    },
};

use crate::spinner::Spinner;

/// Child-process (podman) output: file only `debug!(target: CHILD_TARGET, …)`.
pub const CHILD_TARGET: &str = "dpl::child";

/// Phase transitions: console with spinner `info!(target: PHASE_TARGET, …)`.
pub const PHASE_TARGET: &str = "dpl::phase";

/// Owns a per-deploy `tracing` subscriber and the deploy spinner. Install it as
/// the thread-default dispatcher via [`set_default`](Self::set_default); logging
/// then flows through the `tracing` macros. Writing happens in [`DeployLayer`].
pub struct DeployLog {
    dispatch: Dispatch,
    started: Instant,
    spinner: Spinner,
    log_path: PathBuf,
}

impl DeployLog {
    pub fn open(log_path: &Path, unit: &str, version: u32) -> io::Result<Self> {
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;

        let spinner = Spinner::new(format!("{unit} v{version}: starting"));

        let started = Instant::now();
        let layer = DeployLayer {
            file: Mutex::new(BufWriter::new(file)),
            bar: spinner.bar().clone(),
            started,
        };
        let dispatch = Dispatch::new(Registry::default().with(layer));

        let log = Self {
            dispatch,
            started,
            spinner,
            log_path: log_path.to_path_buf(),
        };
        log.emit(|| tracing::debug!("deploy started: {unit} v{version}"));

        Ok(log)
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Make this deploy's subscriber the thread default. Hold the returned guard
    /// for the deploy's lifetime so the `tracing` macros route here.
    #[must_use]
    pub fn set_default(&self) -> tracing::dispatcher::DefaultGuard {
        tracing::dispatcher::set_default(&self.dispatch)
    }

    /// Route `f` here regardless of the thread default, for the terminal lines
    /// below which must always reach the build log.
    fn emit(&self, f: impl FnOnce()) {
        tracing::dispatcher::with_default(&self.dispatch, f);
    }

    /// Stop the spinner, write the trailing "finished" line.
    pub fn finish_ok(&self) -> Duration {
        let elapsed = self.elapsed();
        self.emit(|| tracing::debug!("finished in {}", fmt_elapsed(elapsed)));
        self.spinner.finish();
        elapsed
    }

    /// Stop the spinner, write the trailing "failed" line, and point the user at
    /// the build log on the console (the error chain itself is reported by the
    /// caller via anyhow).
    pub fn finish_err(&self) -> Duration {
        let elapsed = self.elapsed();
        self.emit(|| tracing::debug!("failed after {}", fmt_elapsed(elapsed)));
        self.spinner.finish();
        eprintln!("Details: {}", self.log_path.display());
        elapsed
    }
}

/// Backs a single deploy: writes every event to the build-log file, and echoes
/// phase/warn/error to the console (above the spinner on a TTY). `dpl::child`
/// output stays file-only.
struct DeployLayer {
    file: Mutex<BufWriter<std::fs::File>>,
    bar: ProgressBar,
    started: Instant,
}

impl DeployLayer {
    fn write_file(&self, line: &str) {
        let mut file = self.file.lock().expect("deploy log file mutex poisoned");
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }

    fn echo(&self, line: &str) {
        crate::spinner::print_above(&self.bar, line.as_bytes());
    }
}

impl<S: Subscriber> Layer<S> for DeployLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = visitor.message;

        let meta = event.metadata();
        let (prefix, to_console) = match meta.target() {
            CHILD_TARGET => ("", false),
            PHASE_TARGET => {
                self.bar.set_message(message.clone());
                ("", true)
            }
            _ => match *meta.level() {
                Level::ERROR => ("ERROR: ", true),
                Level::WARN => ("WARN: ", true),
                _ => ("", false),
            },
        };

        let stamped = format!("[{}] {prefix}{message}", fmt_stamp(self.started.elapsed()));
        self.write_file(&stamped);
        if to_console {
            self.echo(&stamped);
        }
    }
}

/// Process-wide fallback: prints `WARN`/`ERROR` to stderr, ignores the rest.
/// A running deploy's [`DeployLog`] overrides it on that thread, so this only
/// surfaces events emitted outside a deploy (e.g. `DeployStateGuard::drop`).
/// No `max_level_hint` override, so the global level filter stays permissive and
/// a deploy's `debug!` events still reach its file layer.
struct StderrLayer;

impl<S: Subscriber> Layer<S> for StderrLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let prefix = match *event.metadata().level() {
            Level::ERROR => "error",
            Level::WARN => "warning",
            _ => return,
        };
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        eprintln!("{prefix}: {}", visitor.message);
    }
}

/// Install the fallback subscriber. Call once at startup; later calls are no-ops.
pub fn init() {
    let _ = tracing::subscriber::set_global_default(Registry::default().with(StderrLayer));
}

/// Captures an event's implicit `message` field (the format-string body).
#[derive(Default)]
struct MessageVisitor {
    message: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message.clear();
            let _ = write!(self.message, "{value:?}");
        }
    }
}

/// Split a duration into whole `(hours, minutes, seconds)`.
fn hms(d: Duration) -> (u64, u64, u64) {
    let secs = d.as_secs();
    (secs / 3600, (secs % 3600) / 60, secs % 60)
}

fn fmt_stamp(d: Duration) -> String {
    let (h, m, s) = hms(d);
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn fmt_elapsed(d: Duration) -> String {
    let (h, m, s) = hms(d);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_written_to_the_log_file() {
        // With the global fallback installed, a deploy's scoped subscriber must
        // still receive `debug!` events (the global layer must not cap the level).
        init();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log").join("build-3.log");

        let log = DeployLog::open(&path, "web", 3).unwrap();
        {
            let _default = log.set_default();
            tracing::info!(target: PHASE_TARGET, "building image");
            tracing::debug!("running: podman build");
            tracing::debug!(target: CHILD_TARGET, "STEP 1/4: FROM alpine");
            tracing::warn!("export skipped");
            tracing::error!("health check failed");
        }
        log.finish_err();

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();

        // Every event lands in the file (the leading `[mm:ss]` stamp varies).
        assert!(lines.iter().any(|l| l.ends_with("deploy started: web v3")));
        assert!(lines.iter().any(|l| l.ends_with("phase: building image")));
        assert!(lines.iter().any(|l| l.ends_with("running: podman build")));
        assert!(
            lines
                .iter()
                .any(|l| l.ends_with("podman: STEP 1/4: FROM alpine"))
        );
        assert!(lines.iter().any(|l| l.ends_with("WARN: export skipped")));
        assert!(
            lines
                .iter()
                .any(|l| l.ends_with("ERROR: health check failed"))
        );
        assert!(lines.iter().any(|l| l.contains("failed after")));
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
    fn fmt_elapsed_short() {
        assert_eq!(fmt_elapsed(Duration::from_secs(0)), "0s");
        assert_eq!(fmt_elapsed(Duration::from_secs(45)), "45s");
        assert_eq!(fmt_elapsed(Duration::from_secs(83)), "1m23s");
        assert_eq!(fmt_elapsed(Duration::from_secs(3725)), "1h02m05s");
    }
}
