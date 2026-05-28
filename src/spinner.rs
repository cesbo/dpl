use std::{
    fmt,
    io::{
        self,
        IsTerminal,
        Write,
    },
    time::Duration,
};

use indicatif::{
    ProgressBar,
    ProgressDrawTarget,
    ProgressState,
    ProgressStyle,
};

const SPINNER: &[&str] = &["◩", "⬒", "⬔", "◨", "◪", "⬓", "⬕", "◧", "✓"];

/// The db spinner style (`dpl db …`): trailing elapsed, no stamp prefix.
pub fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.cyan} {msg} ({elapsed:.dim})")
        .expect("static spinner template")
        .tick_strings(SPINNER)
}

/// The deploy spinner style: an `[MM:SS]` stamp prefix matching the phase lines
/// printed above it, and no trailing elapsed. The stamp reuses
/// [`crate::log::fmt_stamp`] so the live line aligns with the `[MM:SS] ✓ …`
/// completed-phase lines.
pub fn deploy_style() -> ProgressStyle {
    ProgressStyle::with_template("[{stamp:.dim}] {spinner:.cyan} {msg}")
        .expect("static deploy spinner template")
        .with_key("stamp", |state: &ProgressState, w: &mut dyn fmt::Write| {
            let _ = write!(w, "{}", crate::log::fmt_stamp(state.elapsed()));
        })
        .tick_strings(SPINNER)
}

/// Owns a styled progress spinner and its lifecycle. The single source of bar
/// setup shared by the standalone [`Spinner::run`] helper and [`DeployLog`].
///
/// [`DeployLog`]: crate::log::DeployLog
pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    /// Create a styled spinner showing `msg` on stderr, steady-ticking on a
    /// TTY. Hidden when stderr is not a TTY (no spam in logs / piped output).
    /// Uses the db [`spinner_style`]; [`DeployLog`] picks [`deploy_style`] via
    /// [`with_style`](Self::with_style).
    ///
    /// [`DeployLog`]: crate::log::DeployLog
    pub fn new(msg: impl Into<String>) -> Self {
        Self::with_style(msg, spinner_style())
    }

    /// Like [`new`](Self::new), but with an explicit [`ProgressStyle`].
    pub fn with_style(msg: impl Into<String>, style: ProgressStyle) -> Self {
        let is_tty = io::stderr().is_terminal();
        let target = if is_tty {
            ProgressDrawTarget::stderr()
        } else {
            ProgressDrawTarget::hidden()
        };

        let bar = ProgressBar::with_draw_target(None, target);
        bar.set_style(style);
        bar.set_message(msg.into());
        if is_tty {
            bar.enable_steady_tick(Duration::from_millis(100));
        }

        Self { bar }
    }

    /// The underlying bar, e.g. to wire [`stderr_sink`] for streaming child
    /// output above the spinner.
    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }

    pub fn finish(&self) {
        self.bar.finish_and_clear();
    }

    /// Show a spinner with `msg` while `op` runs, then clear it. Passes the
    /// [`ProgressBar`] to `op` so it can print above the spinner with
    /// `bar.suspend(...)` / `bar.println(...)` while the work runs. Returns
    /// whatever `op` returns, so callers keep using `?` on the result.
    pub fn run<T>(msg: impl Into<String>, op: impl FnOnce(&ProgressBar) -> T) -> T {
        let spinner = Spinner::new(msg);
        let result = op(spinner.bar());
        spinner.finish();
        result
    }
}

/// Print `line` above the spinner, then redraw it.
pub fn print_above(bar: &ProgressBar, line: &[u8]) {
    bar.suspend(|| {
        let mut err = io::stderr().lock();
        let _ = err.write_all(line);
        let _ = err.write_all(b"\n");
    });
}

/// A stderr sink that prints each line above the spinner.
pub fn stderr_sink(bar: &ProgressBar) -> impl FnMut(&[u8]) + Send + '_ {
    move |line: &[u8]| print_above(bar, line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_op_result() {
        let out = Spinner::run("working", |_| 41 + 1);
        assert_eq!(out, 42);
    }
}
