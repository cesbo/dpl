use std::{
    io,
    process::{
        Command,
        Stdio,
    },
};

pub const SYSTEMD_DIR: &str = "/etc/systemd/system";

pub fn enable_service(name: &str) -> io::Result<()> {
    run_systemctl(&["-q", "enable", "--now", name])
}

pub fn disable_service(name: &str) -> io::Result<()> {
    run_systemctl(&["-q", "disable", "--now", name])
}

pub fn stop_service(name: &str) -> io::Result<()> {
    run_systemctl(&["-q", "stop", name])
}

pub fn reload() -> io::Result<()> {
    run_systemctl(&["-q", "daemon-reload"])
}

pub fn reload_service(name: &str) -> io::Result<()> {
    run_systemctl(&["-q", "reload", name])
}

pub fn restart_service(name: &str) -> io::Result<()> {
    run_systemctl(&["-q", "restart", name])
}

/// Returns `true` if the unit is currently active (running).
pub fn is_active(name: &str) -> bool {
    run_systemctl(&["-q", "is-active", name]).is_ok()
}

pub fn run_systemctl(args: &[&str]) -> io::Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("systemctl exited with {status}")))
    }
}
