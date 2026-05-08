use std::{
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
    ProgressStyle,
};

#[derive(Clone)]
pub struct DeployLog {
    inner: Arc<Inner>,
}

struct Inner {
    file: Mutex<BufWriter<std::fs::File>>,
    started: Instant,
    bar: ProgressBar,
    is_tty: bool,
}

impl DeployLog {
    pub fn open(log_path: &Path, unit: &str, version: u32) -> io::Result<Self> {
        let file = OpenOptions::new().append(true).open(log_path)?;
        let file = Mutex::new(BufWriter::new(file));

        let is_tty = io::stderr().is_terminal();
        let target = if is_tty {
            ProgressDrawTarget::stderr()
        } else {
            ProgressDrawTarget::hidden()
        };

        let bar = ProgressBar::with_draw_target(None, target);
        let style = ProgressStyle::with_template("{spinner:.cyan} {wide_msg} ({elapsed_precise})")
            .expect("static spinner template")
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✔"]);
        bar.set_style(style);
        bar.set_message(format!("{unit} v{version}: starting"));
        if is_tty {
            bar.enable_steady_tick(Duration::from_millis(100));
        }

        let inner = Inner {
            file,
            started: Instant::now(),
            bar,
            is_tty,
        };
        let log = Self {
            inner: Arc::new(inner),
        };
        log.write_log(&format!("deploy started: {unit} v{version}"));
        Ok(log)
    }

    pub fn elapsed(&self) -> Duration {
        self.inner.started.elapsed()
    }

    /// Move to a new phase. Updates the spinner, prints the phase line to
    /// console (above spinner in TTY, plain stderr otherwise), and writes to
    /// the log file.
    pub fn phase(&self, phase: &str) {
        let line = self.stamped(&format!("phase: {phase}"));
        self.write_log(&format!("phase: {phase}"));
        self.echo(&line);
        self.inner.bar.set_message(phase.to_string());
    }

    /// Diagnostic line written to the log file only (e.g. command lines).
    pub fn detail(&self, msg: &str) {
        self.write_log(msg);
    }

    /// Captured podman stdout/stderr line.
    pub fn podman_line(&self, line: &str) {
        self.write_log(&format!("podman: {line}"));
    }

    /// Recoverable warning. Echoed to console and log file.
    pub fn warn(&self, msg: &str) {
        let stamped = self.stamped(&format!("WARN: {msg}"));
        self.write_log(&format!("WARN: {msg}"));
        self.echo(&stamped);
    }

    /// Error. Echoed to console and log file.
    pub fn error(&self, msg: &str) {
        let stamped = self.stamped(&format!("ERROR: {msg}"));
        self.write_log(&format!("ERROR: {msg}"));
        self.echo(&stamped);
    }

    /// Stop the spinner, write the trailing "finished" line.
    pub fn finish_ok(&self) -> Duration {
        let elapsed = self.elapsed();
        self.write_log(&format!("finished in {}", fmt_elapsed(elapsed)));
        self.inner.bar.finish_and_clear();
        elapsed
    }

    /// Stop the spinner, write the trailing "failed" line.
    pub fn finish_err(&self, err: &str) -> Duration {
        let elapsed = self.elapsed();
        self.write_log(&format!("failed after {}: {err}", fmt_elapsed(elapsed)));
        self.inner.bar.finish_and_clear();
        elapsed
    }

    fn echo(&self, line: &str) {
        if self.inner.is_tty {
            self.inner.bar.println(line);
        } else {
            eprintln!("{line}");
        }
    }

    fn stamped(&self, msg: &str) -> String {
        format!("[{}] {msg}", fmt_stamp(self.elapsed()))
    }

    fn write_log(&self, msg: &str) {
        let stamped = self.stamped(msg);
        let mut file = self.inner.file.lock().expect("deploy log file mutex poisoned");
        let _ = writeln!(file, "{stamped}");
        let _ = file.flush();
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
