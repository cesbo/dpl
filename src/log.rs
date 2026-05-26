use std::{
    fmt::{
        self,
        Write as _,
    },
    fs::OpenOptions,
    io::{
        self,
        BufWriter,
        IsTerminal,
        Write,
    },
    path::Path,
    sync::{
        Arc,
        Mutex,
    },
    time::{
        Duration,
        Instant,
    },
};

use indicatif::{
    ProgressBar,
    ProgressDrawTarget,
};
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

/// Captured child-process (podman) output: written to the build-log file only,
/// never echoed to the console.
const CHILD_TARGET: &str = "dpl::child";
/// Phase transitions: echoed to the console and used to drive the spinner
/// message.
const PHASE_TARGET: &str = "dpl::phase";

/// Façade over a per-deploy `tracing` subscriber. Each method emits an event
/// routed to the subscriber owned by this log, regardless of the calling thread
/// (podman output is logged from worker threads). The actual file/console
/// writing lives in [`DeployLayer`].
#[derive(Clone)]
pub struct DeployLog {
    inner: Arc<Inner>,
}

struct Inner {
    dispatch: Dispatch,
    started: Instant,
    bar: ProgressBar,
}

impl DeployLog {
    pub fn open(log_path: &Path, unit: &str, version: u32) -> io::Result<Self> {
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(log_path)?;

        let is_tty = io::stderr().is_terminal();
        let target = if is_tty {
            ProgressDrawTarget::stderr()
        } else {
            ProgressDrawTarget::hidden()
        };

        let bar = ProgressBar::with_draw_target(None, target);
        bar.set_style(crate::spinner::spinner_style());
        bar.set_message(format!("{unit} v{version}: starting"));
        if is_tty {
            bar.enable_steady_tick(Duration::from_millis(100));
        }

        let started = Instant::now();
        let layer = DeployLayer {
            file: Mutex::new(BufWriter::new(file)),
            bar: bar.clone(),
            started,
            is_tty,
        };
        let dispatch = Dispatch::new(Registry::default().with(layer));

        let log = Self {
            inner: Arc::new(Inner {
                dispatch,
                started,
                bar,
            }),
        };
        log.detail(&format!("deploy started: {unit} v{version}"));
        Ok(log)
    }

    pub fn elapsed(&self) -> Duration {
        self.inner.started.elapsed()
    }

    /// Route `f` to this deploy's subscriber. Used so the `tracing` macros below
    /// reach the right subscriber even when called from a podman worker thread.
    fn emit(&self, f: impl FnOnce()) {
        tracing::dispatcher::with_default(&self.inner.dispatch, f);
    }

    /// Move to a new phase. Updates the spinner message and echoes the phase
    /// line to the console and the log file.
    pub fn phase(&self, phase: &str) {
        self.emit(|| tracing::info!(target: PHASE_TARGET, "{phase}"));
    }

    /// Diagnostic line written to the log file only (e.g. command lines).
    pub fn detail(&self, msg: &str) {
        self.emit(|| tracing::debug!("{msg}"));
    }

    /// Captured podman stdout/stderr line. Log file only.
    pub fn podman_line(&self, line: &str) {
        self.emit(|| tracing::debug!(target: CHILD_TARGET, "{line}"));
    }

    /// Recoverable warning. Echoed to console and log file.
    pub fn warn(&self, msg: &str) {
        self.emit(|| tracing::warn!("{msg}"));
    }

    /// Error. Echoed to console and log file.
    pub fn error(&self, msg: &str) {
        self.emit(|| tracing::error!("{msg}"));
    }

    /// Stop the spinner, write the trailing "finished" line.
    pub fn finish_ok(&self) -> Duration {
        let elapsed = self.elapsed();
        self.detail(&format!("finished in {}", fmt_elapsed(elapsed)));
        self.inner.bar.finish_and_clear();
        elapsed
    }

    /// Stop the spinner, write the trailing "failed" line.
    pub fn finish_err(&self) -> Duration {
        let elapsed = self.elapsed();
        self.detail(&format!("failed after {}", fmt_elapsed(elapsed)));
        self.inner.bar.finish_and_clear();
        elapsed
    }
}

/// `tracing` layer backing a single deploy: every event is written to the
/// build-log file; phase/warn/error events are also echoed to the console
/// (above the spinner on a TTY), while `dpl::child` output stays file-only.
struct DeployLayer {
    file: Mutex<BufWriter<std::fs::File>>,
    bar: ProgressBar,
    started: Instant,
    is_tty: bool,
}

impl DeployLayer {
    fn write_file(&self, line: &str) {
        let mut file = self.file.lock().expect("deploy log file mutex poisoned");
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }

    fn echo(&self, line: &str) {
        if self.is_tty {
            self.bar.println(line);
        } else {
            eprintln!("{line}");
        }
    }
}

impl<S: Subscriber> Layer<S> for DeployLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        let message = visitor.message;

        let meta = event.metadata();
        let (prefix, to_console) = match meta.target() {
            CHILD_TARGET => ("podman: ", false),
            PHASE_TARGET => {
                self.bar.set_message(message.clone());
                ("phase: ", true)
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

fn fmt_stamp(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn fmt_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log").join("build-3.log");

        let log = DeployLog::open(&path, "web", 3).unwrap();
        log.phase("building image");
        log.detail("running: podman build");
        log.podman_line("STEP 1/4: FROM alpine");
        log.warn("export skipped");
        log.error("health check failed");
        log.finish_err();

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();

        // Every event lands in the file (the leading `[mm:ss]` stamp varies).
        assert!(lines.iter().any(|l| l.ends_with("deploy started: web v3")));
        assert!(lines.iter().any(|l| l.ends_with("phase: building image")));
        assert!(lines.iter().any(|l| l.ends_with("running: podman build")));
        assert!(lines.iter().any(|l| l.ends_with("podman: STEP 1/4: FROM alpine")));
        assert!(lines.iter().any(|l| l.ends_with("WARN: export skipped")));
        assert!(lines.iter().any(|l| l.ends_with("ERROR: health check failed")));
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
