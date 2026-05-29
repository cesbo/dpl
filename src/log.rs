use std::{
    fmt::{
        self,
        Write as _,
    },
    fs::OpenOptions,
    io::{
        self,
        BufRead,
        BufWriter,
        Write,
    },
    path::{
        Path,
        PathBuf,
    },
    sync::{
        Arc,
        Mutex,
    },
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
    span::{
        Attributes,
        Id,
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

/// Drain a child process's output stream to EOF, emitting each line to the
/// build log as a [`CHILD_TARGET`] event (file-only).
///
/// Streams are usually drained on a worker thread that does not inherit the
/// deploy's thread-default subscriber, so capture it with
/// `tracing::dispatcher::get_default` and wrap this call in `with_default` for
/// the lines to reach the log (see `app::podman` and `db::backup`).
pub fn child_output<R: io::Read>(reader: R) {
    let reader = io::BufReader::new(reader);
    for line in reader.lines() {
        let Ok(line) = line else {
            break;
        };
        tracing::debug!(target: CHILD_TARGET, "{line}");
    }
}

/// Name of the spans opened by [`phase`]; how [`DeployLayer`] tells a deploy
/// phase apart from any other span. Must match the literal in [`phase`] (span
/// names are static metadata, so the macro can't reference this constant).
const PHASE_SPAN: &str = "dpl::phase";

pub fn success_mark() -> console::StyledObject<&'static str> {
    console::style("✓").green()
}

pub fn error_mark() -> console::StyledObject<&'static str> {
    console::style("✗").red()
}

pub fn build_log_path(unit_dir: &Path) -> PathBuf {
    unit_dir.join("build.log")
}

/// Open and enter a deploy phase.
/// Sets the spinner message and stamps
/// The previous phase's `✓` line is echoed when the next phase opens.
pub fn phase(message: impl fmt::Display) -> tracing::span::EnteredSpan {
    tracing::info_span!(PHASE_SPAN, message = %message).entered()
}

/// Owns a per-deploy `tracing` subscriber and the deploy spinner. Install it as
/// the thread-default dispatcher via [`set_default`](Self::set_default); logging
/// then flows through the `tracing` macros. Writing happens in [`DeployLayer`].
pub struct DeployLog {
    dispatch: Dispatch,
    started: Instant,
    spinner: Spinner,
    path: PathBuf,
    /// Name of the current phase to print before next phase start.
    phase: Arc<Mutex<Option<String>>>,
}

impl DeployLog {
    pub fn open(path: &Path, unit: &str, version: u32) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;

        let spinner = Spinner::with_style(
            format!("{unit} v{version}: starting"),
            crate::spinner::deploy_style(),
        );
        let started = Instant::now();
        let phase = Arc::new(Mutex::new(None));
        let layer = DeployLayer {
            file: Mutex::new(BufWriter::new(file)),
            bar: spinner.bar().clone(),
            started,
            phase: Arc::clone(&phase),
        };
        let dispatch = Dispatch::new(Registry::default().with(layer));

        let log = Self {
            dispatch,
            started,
            spinner,
            path: path.to_path_buf(),
            phase,
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

    /// Stop the spinner and print the success summary.
    pub fn finish_ok(&self) -> Duration {
        let elapsed = self.elapsed();
        if let Some(prev) = self.phase.lock().expect("phase mutex poisoned").take() {
            echo_phase_done(self.spinner.bar(), self.started, &prev);
        }
        self.spinner.finish();
        eprintln!("[{}] {} deployed.", fmt_stamp(elapsed), success_mark());
        elapsed
    }

    /// Stop the spinner and print the failure summary.
    pub fn finish_err(&self) -> Duration {
        let elapsed = self.elapsed();
        let phase = self
            .phase
            .lock()
            .expect("phase mutex poisoned")
            .take()
            .unwrap_or_else(|| self.spinner.bar().message());
        self.spinner.finish();
        eprintln!(
            "[{}] {} {phase} failed. Log: {}",
            fmt_stamp(elapsed),
            error_mark(),
            self.path.display()
        );
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
    /// Current, not-yet-finished phase name. Shared with [`DeployLog`].
    phase: Arc<Mutex<Option<String>>>,
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
    fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
        if attrs.metadata().name() != PHASE_SPAN {
            return;
        }
        let mut visitor = MessageVisitor::default();
        attrs.record(&mut visitor);
        let message = visitor.message;

        // Record the phase's start stamp in the build-log file.
        self.write_file(&format!(
            "[{}] {message}",
            fmt_stamp(self.started.elapsed())
        ));

        // Leave a completed-phase line for the phase that just ended, then carry
        // the new one on the live spinner.
        let prev = self
            .phase
            .lock()
            .expect("phase mutex poisoned")
            .replace(message.clone());
        self.bar.set_message(message.clone());
        if let Some(prev) = prev {
            echo_phase_done(&self.bar, self.started, &prev);
        }
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = visitor.message;

        let meta = event.metadata();
        let (prefix, to_console) = match meta.target() {
            CHILD_TARGET => ("", false),
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

/// Print a `[mm:ss] ✓ <name>` line for a completed phase above the spinner,
/// leaving it in the terminal while the spinner continues on its own line below.
/// The stamp is cumulative elapsed since deploy start, matching [`fmt_stamp`].
fn echo_phase_done(bar: &ProgressBar, started: Instant, name: &str) {
    let line = format!(
        "[{}] {} {name}",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_are_written_to_the_log_file() {
        // With the global fallback installed, a deploy's scoped subscriber must
        // still receive `debug!` events (the global layer must not cap the level).
        init();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("build.log");

        let log = DeployLog::open(&path, "web", 3).unwrap();
        {
            let _default = log.set_default();
            let _phase = phase("building image");
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
        assert!(lines.iter().any(|l| l.ends_with("building image")));
        assert!(lines.iter().any(|l| l.ends_with("running: podman build")));
        assert!(lines.iter().any(|l| l.ends_with("STEP 1/4: FROM alpine")));
        assert!(lines.iter().any(|l| l.ends_with("WARN: export skipped")));
        assert!(
            lines
                .iter()
                .any(|l| l.ends_with("ERROR: health check failed"))
        );
        assert!(lines.iter().any(|l| l.contains("failed")));
    }

    #[test]
    fn child_output_from_a_spawned_thread_reaches_the_file() {
        // The db restore drains the client's stderr on a separate thread, which
        // does not inherit the deploy's thread-local subscriber. Callers must
        // propagate the dispatcher (as `database.rs` does) for those lines to
        // land in the build log; this guards that the propagation works.
        init();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("build.log");

        let log = DeployLog::open(&path, "db-sezam", 7).unwrap();
        {
            let _default = log.set_default();
            let _phase = phase("restoring database");

            let dispatch = tracing::dispatcher::get_default(|d| d.clone());
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let stream = b"ERROR 1064 (42000): syntax error\n".as_slice();
                    tracing::dispatcher::with_default(&dispatch, || child_output(stream));
                });
            });
        }
        log.finish_err();

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.lines()
                .any(|l| l.ends_with("ERROR 1064 (42000): syntax error")),
            "child stderr emitted from a spawned thread must reach the build log; got:\n{body}"
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
}
