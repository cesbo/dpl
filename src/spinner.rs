use std::{
    io::{
        self,
        IsTerminal,
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
    ProgressStyle::with_template("{spinner:.cyan} {wide_msg} ({elapsed_precise})")
        .expect("static spinner template")
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✔"])
}

/// Show a spinner with `msg` on stderr while `op` runs, then clear it.
/// Hidden when stderr is not a TTY (no spam in logs / piped output).
/// Returns whatever `op` returns, so callers keep using `?` on the result.
pub fn with_spinner<T>(msg: impl Into<String>, op: impl FnOnce() -> T) -> T {
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

    let result = op();
    bar.finish_and_clear();
    result
}
