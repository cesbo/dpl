pub mod health;

use std::{
    io,
    path::PathBuf,
    process::{
        Command,
        Stdio,
    },
};

use serde::Deserialize;

use crate::config::UnitName;

/// Podman volume for app static exports; mounts to [`NGINX_WWW_MOUNT`] in the
/// nginx container. Holds `<name>_<version>/…` directories at its root.
pub const NGINX_WWW_VOLUME: &str = "dpl-www";

/// Mount base of [`NGINX_WWW_VOLUME`] inside the nginx container.
pub const NGINX_WWW_MOUNT: &str = "/var/www";

pub fn podman_spawn_error(err: io::Error) -> io::Error {
    if err.kind() == io::ErrorKind::NotFound {
        io::Error::new(io::ErrorKind::NotFound, "podman not found")
    } else {
        err
    }
}

/// Run podman and capture its trimmed stdout.
pub fn run_podman(args: &[&str]) -> io::Result<String> {
    let output = Command::new("podman")
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map_err(podman_spawn_error)?;

    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    } else {
        Err(io::Error::other(format!(
            "podman exited with {}",
            output.status
        )))
    }
}

/// Create the named volume if it does not already exist.
pub fn ensure_volume(name: &str) -> io::Result<()> {
    if run_podman(&["volume", "exists", name]).is_err() {
        run_podman(&["volume", "create", name])?;
    }

    Ok(())
}

/// Resolve the host mountpoint of a named volume.
pub fn volume_mountpoint(name: &str) -> io::Result<PathBuf> {
    let mountpoint = run_podman(&["volume", "inspect", name, "--format", "{{.Mountpoint}}"])?;
    if mountpoint.is_empty() {
        return Err(io::Error::other(format!("volume {name} has no mountpoint")));
    }
    Ok(PathBuf::from(mountpoint))
}

/// Subset of `podman container inspect` we surface in reports.
#[derive(Debug, Deserialize)]
pub struct ContainerState {
    #[serde(rename = "State")]
    pub state: ContainerStatus,
    #[serde(rename = "RestartCount")]
    pub restart_count: u32,
    #[serde(rename = "ImageName")]
    pub image_name: String,
}

#[derive(Debug, Deserialize)]
pub struct ContainerStatus {
    /// running / exited / created / paused / …
    #[serde(rename = "Status")]
    pub status: String,
    #[serde(rename = "StartedAt")]
    pub started_at: String,
    /// Zero-value (`0001-01-01T00:00:00Z`) while the container is still running.
    #[serde(rename = "FinishedAt", default)]
    pub finished_at: String,
    #[serde(rename = "ExitCode")]
    pub exit_code: i32,
}

/// Inspect a container by name. `None` when it does not exist (podman
/// inspect exits non-zero)
pub fn inspect_container(name: &UnitName) -> Option<ContainerState> {
    let name = name.scoped_unit_name();
    let out = run_podman(&["container", "inspect", &name, "--format", "{{json .}}"]).ok()?;
    let state = serde_json::from_str(&out).ok()?;
    Some(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_error_clarifies_missing_podman() {
        let mapped = podman_spawn_error(io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(mapped.kind(), io::ErrorKind::NotFound);
        assert!(mapped.to_string().contains("podman not found"), "{mapped}");

        // Unrelated spawn failures pass through untouched.
        let other = podman_spawn_error(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
        assert_eq!(other.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(other.to_string(), "denied");
    }

    #[test]
    fn parses_container_inspect_json() {
        let json = r#"{
            "Id": "abc123",
            "State": {
                "Status": "running",
                "StartedAt": "2026-05-20T10:11:12.123456789Z",
                "FinishedAt": "0001-01-01T00:00:00Z",
                "ExitCode": 0
            },
            "RestartCount": 2,
            "ImageName": "localhost/dpl-web:3"
        }"#;

        let state: ContainerState = serde_json::from_str(json).unwrap();
        assert_eq!(state.state.status, "running");
        assert_eq!(state.state.started_at, "2026-05-20T10:11:12.123456789Z");
        assert_eq!(state.state.finished_at, "0001-01-01T00:00:00Z");
        assert_eq!(state.state.exit_code, 0);
        assert_eq!(state.restart_count, 2);
        assert_eq!(state.image_name, "localhost/dpl-web:3");
    }
}
