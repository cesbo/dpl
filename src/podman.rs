use std::{
    io,
    path::PathBuf,
    process::{
        Command,
        Stdio,
    },
};

/// Run podman and capture its trimmed stdout.
pub fn run_podman(args: &[&str]) -> io::Result<String> {
    let output = Command::new("podman")
        .args(args)
        .stderr(Stdio::null())
        .output()?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(io::Error::other(format!(
            "podman exited with {}",
            output.status
        )))
    }
}

/// Resolve the host mountpoint of a named volume.
pub fn volume_mountpoint(name: &str) -> io::Result<PathBuf> {
    let mountpoint = run_podman(&["volume", "inspect", name, "--format", "{{.Mountpoint}}"])?;
    if mountpoint.is_empty() {
        return Err(io::Error::other(format!("volume {name} has no mountpoint")));
    }
    Ok(PathBuf::from(mountpoint))
}
