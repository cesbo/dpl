use std::{
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
    ProgressStyle,
};

/// The cyan braille spinner style shared by `DeployLog` and `with_spinner`.
pub fn spinner_style() -> ProgressStyle {
    ProgressStyle::with_template("{spinner:.cyan}{wide_msg} ({elapsed_precise})")
        .expect("static spinner template")
        .tick_strings(&[
            "⢀⠀", "⡀⠀", "⠄⠀", "⢂⠀", "⡂⠀", "⠅⠀", "⢃⠀", "⡃⠀", "⠍⠀", "⢋⠀", "⡋⠀", "⠍⠁", "⢋⠁", "⡋⠁",
            "⠍⠉", "⠋⠉", "⠋⠉", "⠉⠙", "⠉⠙", "⠉⠩", "⠈⢙", "⠈⡙", "⢈⠩", "⡀⢙", "⠄⡙", "⢂⠩", "⡂⢘", "⠅⡘",
            "⢃⠨", "⡃⢐", "⠍⡐", "⢋⠠", "⡋⢀", "⠍⡁", "⢋⠁", "⡋⠁", "⠍⠉", "⠋⠉", "⠋⠉", "⠉⠙", "⠉⠙", "⠉⠩",
            "⠈⢙", "⠈⡙", "⠈⠩", "⠀⢙", "⠀⡙", "⠀⠩", "⠀⢘", "⠀⡘", "⠀⠨", "⠀⢐", "⠀⡐", "⠀⠠", "⠀⢀", "⠀⡀",
            "+",
        ])
}

/// Owns a styled progress spinner and its lifecycle. The single source of bar
/// setup shared by the standalone [`Spinner::run`] helper and [`DeployLog`].
///
/// [`DeployLog`]: crate::log::DeployLog
pub struct Spinner {
    bar: ProgressBar,
    is_tty: bool,
}

impl Spinner {
    /// Create a styled spinner showing `msg` on stderr, steady-ticking on a
    /// TTY. Hidden when stderr is not a TTY (no spam in logs / piped output).
    pub fn new(msg: impl Into<String>) -> Self {
        let is_tty = io::stderr().is_terminal();
        let target = if is_tty {
            ProgressDrawTarget::stderr()
        } else {
            ProgressDrawTarget::hidden()
        };

        let bar = ProgressBar::with_draw_target(None, target);
        bar.set_style(spinner_style());
        bar.set_message(msg.into());
        if is_tty {
            bar.enable_steady_tick(Duration::from_millis(100));
        }

        Self { bar, is_tty }
    }

    /// The underlying bar, e.g. to wire [`stderr_sink`] for streaming child
    /// output above the spinner.
    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }

    pub fn is_tty(&self) -> bool {
        self.is_tty
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

/// A stderr sink that prints each line above the spinner.
pub fn stderr_sink(bar: &ProgressBar) -> impl FnMut(&[u8]) + Send + '_ {
    move |line: &[u8]| {
        bar.suspend(|| {
            let mut err = io::stderr().lock();
            let _ = err.write_all(line);
            let _ = err.write_all(b"\n");
        });
    }
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
