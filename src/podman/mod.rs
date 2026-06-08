pub mod health;
pub mod inspect;
mod run;

use std::{
    fs,
    io,
    path::Path,
    process::{
        Command,
        Stdio,
    },
};

use serde::Deserialize;

pub use self::run::PodmanRun;
use crate::config::UnitName;

/// The shared podman network every dpl container joins.
pub const NETWORK: &str = "dpl";

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

/// Stop and remove a unit's container. Idempotent: `--ignore` makes a missing
/// container a no-op.
pub fn stop_and_remove(name: &UnitName) -> io::Result<()> {
    let container = name.scoped_unit_name();
    run_podman(&["stop", "--ignore", &container])?;
    run_podman(&["rm", "-f", "-v", "--ignore", &container])?;
    Ok(())
}

/// Create the named volume if it does not already exist.
pub fn ensure_volume(name: &str) -> io::Result<()> {
    if run_podman(&["volume", "exists", name]).is_err() {
        run_podman(&["volume", "create", name])?;
    }

    Ok(())
}

/// Write a file into a named volume.
pub fn write_volume_file(
    volume: &str,
    image: &str,
    path: &str,
    content: impl AsRef<[u8]>,
) -> io::Result<()> {
    let rel_path = normalize_volume_file_path(path)?;
    let temp_dir = tempfile::tempdir()?;
    let local_file = temp_dir.path().join(&rel_path);
    if let Some(parent) = local_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&local_file, content)?;

    copy_host_dir_to_volume(volume, image, temp_dir.path())
}

/// Copy a directory from an image into a named volume.
pub fn copy_image_dir_to_volume(
    image: &str,
    source_dir: &str,
    volume: &str,
    dest_dir: &str,
) -> io::Result<()> {
    let source_dir = normalize_container_dir_path(source_dir)?;
    let dest_dir = normalize_volume_dir_path(dest_dir)?;
    let source_container = temporary_volume_write_container();
    let temp_dir = tempfile::tempdir()?;
    let local_dest = temp_dir.path().join(&dest_dir);
    fs::create_dir_all(&local_dest)?;

    run_podman(&["create", "--name", &source_container, image])?;

    let src = format!("{source_container}:{source_dir}/.");
    let dst = local_dest.display().to_string();
    let copy_result = run_podman(&["cp", &src, &dst]);
    let remove_result = run_podman(&["rm", "-f", "-v", "--ignore", &source_container]);

    copy_result?;
    remove_result?;
    copy_host_dir_to_volume(volume, image, temp_dir.path())
}

/// Copy a directory from a host into a named volume.
fn copy_host_dir_to_volume(volume: &str, image: &str, source_dir: &Path) -> io::Result<()> {
    ensure_volume(volume)?;

    let container = temporary_volume_write_container();
    let mount = Path::new("/tmp").join(&container).display().to_string();
    let volume_arg = format!("{volume}:{mount}");

    run_podman(&[
        "create",
        "--name",
        &container,
        "--volume",
        &volume_arg,
        image,
    ])?;

    let src = format!("{}/.", source_dir.display());
    let dst = format!("{container}:{mount}");
    let copy_result = run_podman(&["cp", "--overwrite", &src, &dst]);
    let remove_result = run_podman(&["rm", "-f", "-v", "--ignore", &container]);

    copy_result?;
    remove_result?;
    Ok(())
}

fn temporary_volume_write_container() -> String {
    format!("dpl-volume-write-{}", cuid::cuid2())
}

fn normalize_container_dir_path(path: &str) -> io::Result<String> {
    let normalized = path.trim_end_matches('/');
    if normalized.is_empty() || !normalized.starts_with('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "container directory path must be absolute and not root",
        ));
    }

    Ok(normalized.to_string())
}

fn normalize_volume_file_path(path: &str) -> io::Result<String> {
    let normalized = normalize_volume_dir_path(path)?;
    if normalized.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "volume file path must not be empty",
        ));
    }

    Ok(normalized)
}

fn normalize_volume_dir_path(path: &str) -> io::Result<String> {
    if path.is_empty() || path.starts_with('/') || path.contains('\'') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "volume path must be relative and must not contain single quotes",
        ));
    }

    let mut normalized = String::new();
    for item in path.split('/') {
        if item.is_empty() || item == "." || item == ".." {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "volume path must not contain empty, current or parent segments",
            ));
        }

        if !normalized.is_empty() {
            normalized.push('/');
        }
        normalized.push_str(item);
    }

    Ok(normalized)
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
    /// Writable-layer size in bytes.
    /// How much the container has grown on top of its image.
    #[serde(rename = "SizeRw", default)]
    pub size_rw: Option<i64>,
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

/// Inspect a container by name.
/// `None` when it does not exist (podman inspect exits non-zero).
/// `sized` to compute the writeable-layer size, used for the inspect report
pub fn inspect_container(name: &UnitName, sized: bool) -> Option<ContainerState> {
    let name = name.scoped_unit_name();
    let mut args = vec!["container", "inspect", &name, "--format", "{{json .}}"];
    if sized {
        args.push("--size");
    }
    let out = run_podman(&args).ok()?;
    serde_json::from_str(&out).ok()
}

/// Returns `true` if the unit's container is currently running.
pub fn is_running(name: &UnitName) -> bool {
    inspect_container(name, false)
        .map(|c| c.state.status == "running")
        .unwrap_or(false)
}

/// Live resource usage from `podman stats --no-stream`. The fields are
/// podman's own pre-formatted strings (e.g. `"0.50%"`, `"12.3MB / 4.0GB"`),
/// surfaced verbatim.
#[derive(Debug)]
pub struct ContainerStats {
    pub cpu_perc: String,
    pub mem_usage: String,
    pub mem_perc: String,
    pub net_io: String,
}

/// Sample live resource stats for a running container. `None` when the
/// container is not running or stats are unavailable (e.g. rootless cgroup v1).
pub fn container_stats(name: &UnitName) -> Option<ContainerStats> {
    let out = run_podman(&[
        "stats",
        "--no-stream",
        "--format",
        "{{.CPUPerc}}\t{{.MemUsage}}\t{{.MemPerc}}\t{{.NetIO}}",
        &name.scoped_unit_name(),
    ])
    .ok()?;
    parse_stats(&out)
}

/// Parse a single tab-separated `podman stats --format` row. Pure, so the
/// field layout can be tested without spawning podman.
fn parse_stats(out: &str) -> Option<ContainerStats> {
    let mut f = out.split('\t');
    Some(ContainerStats {
        cpu_perc: f.next()?.trim().to_string(),
        mem_usage: f.next()?.trim().to_string(),
        mem_perc: f.next()?.trim().to_string(),
        net_io: f.next()?.trim().to_string(),
    })
}

/// Size of an image in bytes, via `podman image inspect`. `None` when the image
/// is unknown or the size is unparseable.
pub fn image_size(image: &str) -> Option<u64> {
    run_podman(&["image", "inspect", image, "--format", "{{.Size}}"])
        .ok()?
        .parse()
        .ok()
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
        // No `--size`, so the writable layer is absent.
        assert_eq!(state.size_rw, None);
    }

    #[test]
    fn parses_sized_container_inspect_json() {
        // `podman container inspect --size` adds SizeRw (writable layer bytes).
        let json = r#"{
            "State": {
                "Status": "running",
                "StartedAt": "2026-05-20T10:11:12Z",
                "ExitCode": 0
            },
            "RestartCount": 0,
            "ImageName": "localhost/dpl-web:3",
            "SizeRw": 4718592,
            "SizeRootFs": 215257088
        }"#;

        let state: ContainerState = serde_json::from_str(json).unwrap();
        assert_eq!(state.size_rw, Some(4_718_592));
    }

    #[test]
    fn parses_stats_row() {
        let s = parse_stats("0.50%\t12.3MB / 4.0GB\t0.30%\t1.2kB / 800B").unwrap();
        assert_eq!(s.cpu_perc, "0.50%");
        assert_eq!(s.mem_usage, "12.3MB / 4.0GB");
        assert_eq!(s.mem_perc, "0.30%");
        assert_eq!(s.net_io, "1.2kB / 800B");

        // A short/garbled row (fewer columns than expected) yields `None`.
        assert!(parse_stats("0.50%\t12.3MB / 4.0GB").is_none());
    }

    #[test]
    fn normalizes_volume_file_path() {
        assert_eq!(
            normalize_volume_file_path("nginx/example.conf").unwrap(),
            "nginx/example.conf"
        );
        assert!(normalize_volume_file_path("/example.conf").is_err());
        assert!(normalize_volume_file_path("nginx//example.conf").is_err());
        assert!(normalize_volume_file_path("nginx/../example.conf").is_err());
        assert!(normalize_volume_file_path("bad'name.conf").is_err());
    }

    #[test]
    fn normalizes_volume_dir_path() {
        assert_eq!(
            normalize_volume_dir_path("web_3/static").unwrap(),
            "web_3/static"
        );
        assert!(normalize_volume_dir_path("").is_err());
        assert!(normalize_volume_dir_path("/web_3/static").is_err());
        assert!(normalize_volume_dir_path("web_3//static").is_err());
        assert!(normalize_volume_dir_path("web_3/../static").is_err());
    }

    #[test]
    fn normalizes_container_dir_path() {
        assert_eq!(
            normalize_container_dir_path("/app/static").unwrap(),
            "/app/static"
        );
        assert_eq!(
            normalize_container_dir_path("/app/static/").unwrap(),
            "/app/static"
        );
        assert!(normalize_container_dir_path("").is_err());
        assert!(normalize_container_dir_path("/").is_err());
        assert!(normalize_container_dir_path("app/static").is_err());
    }
}
