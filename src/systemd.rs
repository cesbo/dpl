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
    run_systemctl(&["-q", "disable", "--now", &name])
}

pub fn start_service(name: &str) -> io::Result<()> {
    run_systemctl(&["-q", "start", name])
}

pub fn stop_service(name: &str) -> io::Result<()> {
    run_systemctl(&["-q", "stop", name])
}

pub fn reload() -> io::Result<()> {
    run_systemctl(&["-q", "daemon-reload"])
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
