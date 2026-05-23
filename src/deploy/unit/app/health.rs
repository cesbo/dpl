use std::{
    io,
    thread::sleep,
    time::Duration,
};

use crate::podman::run_podman;

const ATTEMPTS: usize = 30;
const INTERVAL: Duration = Duration::from_millis(800);

/// Wait until the container has a listening TCP socket on `port`.
pub fn check(name: &str, port: u16) -> io::Result<()> {
    let container = format!("dpl-{name}");
    let port = format!("{port}");

    for _ in 0 .. ATTEMPTS {
        sleep(INTERVAL);

        let success = run_podman(&[
            "exec",
            &container,
            "sh",
            "/opt/dpl/run.sh",
            "check-tcp",
            &port,
        ])
        .is_ok();

        if success {
            return Ok(());
        }
    }

    Err(io::Error::other("timed out".to_string()))
}
