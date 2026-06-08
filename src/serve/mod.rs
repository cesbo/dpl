mod supervisor;
mod tick;
mod timer;

use std::process::Command;

pub use self::{
    supervisor::Supervisor,
    tick::tick,
    timer::run_timer,
};
use crate::MainContext;

pub fn notify(ctx: &MainContext) {
    let Ok(content) = std::fs::read_to_string(ctx.serve_pid_path()) else {
        return;
    };
    let Ok(pid) = content.trim().parse::<u32>() else {
        return;
    };
    let _ = Command::new("kill")
        .arg("-HUP")
        .arg(pid.to_string())
        .status();
}
