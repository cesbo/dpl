pub mod env;
pub mod health;
pub mod inspect;
mod run;

use std::{
    fs,
    io::{
        self,
        Read,
        Seek,
        SeekFrom,
    },
    path::Path,
    process::{
        Child,
        Command,
        ExitStatus,
        Stdio,
    },
    thread::sleep,
    time::{
        Duration,
        Instant,
    },
};

use serde::Deserialize;

pub use self::run::PodmanRun;
use crate::config::UnitName;

/// The shared podman network every dpl container joins.
pub const NETWORK: &str = "dpl";

/// Poll granularity of the bounded waiter.
const RUN_POLL: Duration = Duration::from_millis(50);

/// Default cap for control-plane calls: `exists`, `inspect`, `exec`, `network`,
/// `stats`. These read or write metadata and answer in milliseconds on a healthy
/// host, so a call still running after this has wedged, not slowed down.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);

/// Cap for `stop` / `rm`, which legitimately wait out a container's own stop
/// grace (plus, for a database, its shutdown checkpoint).
pub const STOP_TIMEOUT: Duration = Duration::from_secs(120);

/// Cap for calls that move real data: `pull`, `cp`, the DNS-probe `run`. Only
/// here to make "forever" impossible, never to fail a healthy deploy.
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(1800);

/// Cap for the calls that only feed the `dpl inspect` report. Inspect is what an
/// operator reaches for when something is already wedged, so it has to degrade
/// fast rather than answer late.
pub const REPORT_TIMEOUT: Duration = Duration::from_secs(5);

/// Mount point inside the nginx container of an http-server's per-instance www
/// host dir; holds each served app's `<name>_<version>/...` export tree.
pub const NGINX_WWW_MOUNT: &str = "/var/www";

pub fn podman_spawn_error(err: io::Error) -> io::Error {
    if err.kind() == io::ErrorKind::NotFound {
        io::Error::new(io::ErrorKind::NotFound, "podman not found")
    } else {
        err
    }
}

/// True for an error that means podman itself never answered, as opposed to
/// podman answering "no such container".
pub fn is_unresponsive(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::NotFound
    )
}

/// Wait for `child` up to `limit`; on expiry kill and reap it, then fail with
/// [`io::ErrorKind::TimedOut`] naming `what`.
///
/// Only the podman process is killed - its conmon/container descendants are
/// deliberately left alone, since container lifecycle belongs to `dpl serve`.
/// Never pass a child whose piped stdio the caller is not draining: a full pipe
/// keeps it alive past the deadline.
pub fn wait_within(child: &mut Child, limit: Duration, what: &str) -> io::Result<ExitStatus> {
    let deadline = Instant::now() + limit;

    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("{what} did not return within {}s", limit.as_secs()),
            ));
        }

        sleep(RUN_POLL);
    }
}

/// Run `cmd` under `limit` and capture its trimmed stdout.
///
/// stdout/stderr land in temp files rather than pipes: killing a wedged podman
/// does not necessarily close the write end (a descendant may hold it), so a
/// pipe read could outlive the deadline we just enforced. Control-plane output
/// is small, so the temp files cost nothing.
fn capture_within(cmd: &mut Command, limit: Duration, what: &str) -> io::Result<String> {
    let mut out = tempfile::tempfile()?;
    let mut err = tempfile::tempfile()?;

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::from(out.try_clone()?))
        .stderr(Stdio::from(err.try_clone()?))
        .spawn()
        .map_err(podman_spawn_error)?;

    let status = wait_within(&mut child, limit, what)?;

    if status.success() {
        read_back(&mut out)
    } else {
        let stderr = read_back(&mut err).unwrap_or_default();
        Err(io::Error::other(format!(
            "podman exited with {status}: {stderr}"
        )))
    }
}

/// Rewind a captured-output temp file and read it as trimmed text.
fn read_back(file: &mut fs::File) -> io::Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut buf = String::new();
    file.read_to_string(&mut buf)?;
    Ok(buf.trim().to_string())
}

/// Run podman under an explicit wall-clock cap and capture its trimmed stdout.
pub fn run_podman_within(args: &[&str], limit: Duration) -> io::Result<String> {
    let mut cmd = Command::new("podman");
    cmd.args(args);
    capture_within(&mut cmd, limit, &format!("podman {}", args.join(" ")))
}

/// Run podman under [`CONTROL_TIMEOUT`] and capture its trimmed stdout.
pub fn run_podman(args: &[&str]) -> io::Result<String> {
    run_podman_within(args, CONTROL_TIMEOUT)
}

/// Ensure the shared `dpl` network exists.
pub fn ensure_network(name: &str) -> io::Result<()> {
    if !network_exists(name) {
        run_podman(&["network", "create", name])?;
    }

    Ok(())
}

/// Stop and remove a unit's container.
pub fn stop_and_remove(name: &UnitName) -> io::Result<()> {
    let container = name.scoped_unit_name();
    if container_exists(&container) {
        run_podman_within(&["stop", &container], STOP_TIMEOUT)?;
        run_podman_within(&["rm", "--force", "--volumes", &container], STOP_TIMEOUT)?;
    }

    Ok(())
}

/// Check whether the network exists.
pub fn network_exists(name: &str) -> bool {
    run_podman(&["network", "exists", name]).is_ok()
}

/// Check whether the container exists.
pub fn container_exists(name: &str) -> bool {
    run_podman(&["container", "exists", name]).is_ok()
}

/// Check whether the image exists.
pub fn image_exists(name: &str) -> bool {
    run_podman(&["image", "exists", name]).is_ok()
}

/// Pull the named image into local storage.
pub fn pull_image(image: &str) -> io::Result<()> {
    run_podman_within(&["pull", image], TRANSFER_TIMEOUT)?;
    Ok(())
}

/// Address of podman's embedded DNS.
pub fn detect_network_dns(image: &str) -> io::Result<String> {
    ensure_network(NETWORK)?;
    // May pull the image first, so it gets the transfer budget.
    let resolv = run_podman_within(
        &[
            "run",
            "--rm",
            "--network",
            NETWORK,
            "--entrypoint",
            "cat",
            image,
            "/etc/resolv.conf",
        ],
        TRANSFER_TIMEOUT,
    )?;

    parse_first_nameserver(&resolv)
        .ok_or_else(|| io::Error::other("no IPv4 nameserver in container /etc/resolv.conf"))
}

/// First IPv4 `nameserver` entry in a `resolv.conf` body.
fn parse_first_nameserver(resolv: &str) -> Option<String> {
    resolv
        .lines()
        .filter_map(|line| line.trim().strip_prefix("nameserver"))
        .map(str::trim)
        .find(|ip| !ip.is_empty() && !ip.contains(':'))
        .map(str::to_string)
}

/// Extract `source_dir` from `image` into host `dest_dir` (created if missing).
/// Spawns a throwaway container and `podman cp`s the directory contents out.
pub fn copy_image_dir_to_host(image: &str, source_dir: &str, dest_dir: &Path) -> io::Result<()> {
    let source_dir = normalize_container_dir_path(source_dir)?;
    fs::create_dir_all(dest_dir)?;

    let container = format!("dpl-export-{}", cuid::cuid2());
    run_podman(&["create", "--name", &container, image])?;

    let src = format!("{container}:{source_dir}/.");
    let dst = dest_dir.display().to_string();
    let copy_result = run_podman_within(&["cp", &src, &dst], TRANSFER_TIMEOUT);
    let _ = run_podman(&["rm", &container]);

    copy_result?;
    Ok(())
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

/// Inspect a container by name under an explicit cap.
/// `Ok(None)` when it does not exist; `Err` only when podman itself did not
/// answer, so a caller can tell "no such container" from "podman is wedged".
/// `sized` to compute the writeable-layer size, used for the inspect report.
pub fn try_inspect_container(
    name: &UnitName,
    sized: bool,
    limit: Duration,
) -> io::Result<Option<ContainerState>> {
    let name = name.scoped_unit_name();
    let mut args = vec!["container", "inspect", &name, "--format", "{{json .}}"];
    if sized {
        args.push("--size");
    }

    match run_podman_within(&args, limit) {
        Ok(out) => Ok(serde_json::from_str(&out).ok()),
        Err(err) if is_unresponsive(&err) => Err(err),
        Err(_) => Ok(None),
    }
}

/// Whether the unit's container is running, under an explicit cap.
/// `Err` means podman did not answer - the caller must not read that as "down".
pub fn is_running_within(name: &UnitName, limit: Duration) -> io::Result<bool> {
    Ok(try_inspect_container(name, false, limit)?.is_some_and(|c| c.state.status == "running"))
}

/// Returns `true` if the unit's container is currently running.
pub fn is_running(name: &UnitName) -> bool {
    is_running_within(name, CONTROL_TIMEOUT).unwrap_or(false)
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
    let out = run_podman_within(
        &[
            "stats",
            "--no-stream",
            "--format",
            "{{.CPUPerc}}\t{{.MemUsage}}\t{{.MemPerc}}\t{{.NetIO}}",
            &name.scoped_unit_name(),
        ],
        REPORT_TIMEOUT,
    )
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
    run_podman_within(
        &["image", "inspect", image, "--format", "{{.Size}}"],
        REPORT_TIMEOUT,
    )
    .ok()?
    .parse()
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `capture_within` takes the `Command`, so the bounded-wait behaviour can
    /// be tested with `sh` and needs no podman on the host.
    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn capture_within_returns_trimmed_stdout() {
        let out = capture_within(&mut sh("echo hello"), Duration::from_secs(5), "sh").unwrap();
        assert_eq!(out, "hello");
    }

    #[test]
    fn capture_within_reports_stderr_on_failure() {
        let err = capture_within(
            &mut sh("echo boom 1>&2; exit 3"),
            Duration::from_secs(5),
            "sh",
        )
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("boom"), "{msg}");
        assert!(msg.contains('3'), "{msg}");
    }

    #[test]
    fn capture_within_kills_a_hung_child() {
        let started = Instant::now();
        let err = capture_within(
            &mut sh("sleep 30"),
            Duration::from_millis(200),
            "podman exec",
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(is_unresponsive(&err));
        assert!(err.to_string().contains("podman exec"), "{err}");
        // The point of the timeout: it returns instead of waiting out the child.
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "did not give up"
        );
    }

    #[test]
    fn wait_within_reaps_a_fast_child() {
        let mut child = sh("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let status = wait_within(&mut child, Duration::from_secs(5), "sh").unwrap();
        assert!(status.success());
    }

    #[test]
    fn unresponsive_only_for_no_answer() {
        // A wedged call and a missing binary mean podman never answered.
        assert!(is_unresponsive(&io::Error::from(io::ErrorKind::TimedOut)));
        assert!(is_unresponsive(&io::Error::from(io::ErrorKind::NotFound)));
        // "no such container" is podman answering, and must stay distinguishable.
        assert!(!is_unresponsive(&io::Error::other(
            "podman exited with 125"
        )));
    }

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
    fn parses_first_ipv4_nameserver() {
        let resolv = "search dns.podman\nnameserver 10.89.0.1\nnameserver 8.8.8.8\n";
        assert_eq!(parse_first_nameserver(resolv).as_deref(), Some("10.89.0.1"));

        // IPv6 nameservers are skipped (resolver line is rendered with ipv6=off).
        let v6_first = "nameserver fd00::1\nnameserver 10.89.0.1\n";
        assert_eq!(
            parse_first_nameserver(v6_first).as_deref(),
            Some("10.89.0.1")
        );

        // No usable nameserver yields None.
        assert_eq!(parse_first_nameserver("search foo\n"), None);
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
