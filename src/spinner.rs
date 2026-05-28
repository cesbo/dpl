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

/// Owns a styled progress spinner and its lifecycle. Used by [`DeployLog`] to
/// drive the live `[MM:SS] {spinner} {phase}` line shown during a deploy.
///
/// [`DeployLog`]: crate::log::DeployLog
pub struct Spinner {
    bar: ProgressBar,
}

impl Spinner {
    /// Create a styled spinner showing `msg` on stderr, steady-ticking on a
    /// TTY. Hidden when stderr is not a TTY (no spam in logs / piped output).
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

    /// The underlying bar, e.g. to print above the spinner via
    /// [`print_above`].
    pub fn bar(&self) -> &ProgressBar {
        &self.bar
    }

    pub fn finish(&self) {
        self.bar.finish_and_clear();
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

