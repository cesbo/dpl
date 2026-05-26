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
/// never echoed to the console. Emit with `tracing::debug!(target: CHILD_TARGET, …)`.
pub(crate) const CHILD_TARGET: &str = "dpl::child";
/// Phase transitions: echoed to the console and used to drive the spinner
/// message. Emit with `tracing::info!(target: PHASE_TARGET, …)`.
pub(crate) const PHASE_TARGET: &str = "dpl::phase";

/// Owns a per-deploy `tracing` subscriber and the deploy spinner. Install it as
/// the thread-default dispatcher for the duration of a deploy with
/// [`set_default`](Self::set_default); all logging then flows through the
/// `tracing` macros (phases via `target: PHASE_TARGET`, child output via
/// `target: CHILD_TARGET`). The actual file/console writing lives in
/// [`DeployLayer`].
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
        log.emit(|| tracing::debug!("deploy started: {unit} v{version}"));
        Ok(log)
    }

    pub fn elapsed(&self) -> Duration {
        self.inner.started.elapsed()
    }

    /// Install this deploy's subscriber as the current thread's default
    /// dispatcher. The returned guard restores the previous default on drop;
    /// hold it for the lifetime of the deploy so the `tracing` macros route here.
    #[must_use]
    pub fn set_default(&self) -> tracing::dispatcher::DefaultGuard {
        tracing::dispatcher::set_default(&self.inner.dispatch)
    }

    /// Route `f` to this deploy's subscriber regardless of the current thread
    /// default. Used for the terminal lines below, which must always land in the
    /// build log.
    fn emit(&self, f: impl FnOnce()) {
        tracing::dispatcher::with_default(&self.inner.dispatch, f);
    }

    /// Stop the spinner, write the trailing "finished" line.
    pub fn finish_ok(&self) -> Duration {
        let elapsed = self.elapsed();
        self.emit(|| tracing::debug!("finished in {}", fmt_elapsed(elapsed)));
        self.inner.bar.finish_and_clear();
        elapsed
    }

    /// Stop the spinner, write the trailing "failed" line.
    pub fn finish_err(&self) -> Duration {
        let elapsed = self.elapsed();
        self.emit(|| tracing::debug!("failed after {}", fmt_elapsed(elapsed)));
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

/// Process-wide fallback subscriber: prints `WARN`/`ERROR` events to stderr and
/// ignores everything else. A running deploy installs its own [`DeployLog`] as
/// the thread-default dispatcher ([`DeployLog::set_default`]), which overrides
/// this layer on that thread, so this only surfaces diagnostics emitted outside
/// a deploy scope (e.g. lock-file cleanup in `DeployStateGuard::drop`).
///
/// It deliberately does not override `max_level_hint`, leaving the global level
/// filter permissive so a deploy's `debug!` events still reach its file layer.
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

/// Install the process-wide fallback subscriber. Call once at startup; a second
/// call is a no-op.
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
        // Install the global fallback subscriber too: a deploy's scoped
        // subscriber must still receive `debug!` events, i.e. the global layer
        // must not cap the level filter.
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
