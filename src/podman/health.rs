use std::{
    net::Ipv4Addr,
    thread::sleep,
    time::Duration,
};

use chrono::DateTime;
use thiserror::Error;

use crate::{
    config::UnitName,
    podman::{
        inspect_container,
        run_podman,
    },
};

const ATTEMPTS: usize = 30;
const INTERVAL: Duration = Duration::from_millis(800);

/// Why a [`check`] gave up waiting for the app's port. Carries the diagnosis so
/// the caller can show it and persist it to `.state.json`.
#[derive(Debug, Error)]
pub enum HealthCheckError {
    /// The container is no longer running — it exited before opening the port.
    #[error("container exited with code {exit_code} (ran {uptime})")]
    Exited { exit_code: i32, uptime: String },

    /// The container is running but has no listening TCP sockets at all.
    #[error("container is running but not listening on any port (expected {expected} on 0.0.0.0)")]
    NoPorts { expected: u16 },

    /// The container is listening, but not on the expected port bound to
    /// `0.0.0.0` (e.g. `127.0.0.1:<port>`, or a different port entirely).
    #[error("port {expected} not reachable on 0.0.0.0; found {}", .found.join(", "))]
    WrongBinding { expected: u16, found: Vec<String> },

    /// Timed out and the container state could not be determined.
    #[error("timed out waiting for port {expected} on 0.0.0.0")]
    Timeout { expected: u16 },
}

/// Wait until the container has a listening TCP socket on `port` bound to
/// `0.0.0.0`. On failure, diagnose *why* (crashed, no ports, wrong binding).
pub fn check(name: &UnitName, port: u16) -> Result<(), HealthCheckError> {
    let container = name.scoped_unit_name();

    for _ in 0 .. ATTEMPTS {
        sleep(INTERVAL);

        match run_podman(&["exec", &container, "cat", "/proc/net/tcp"]) {
            Ok(table) if is_ready(&table, port) => return Ok(()),
            Ok(_) => {}
            // exec failed: the container may have exited. Fail fast if so,
            // rather than waiting out the whole window on a crash-looping app.
            Err(_) => {
                if let Some(err) = exited(name) {
                    return Err(err);
                }
            }
        }
    }

    Err(diagnose(name, &container, port))
}

/// `true` when `table` has a TCP_LISTEN socket on `port` bound to `0.0.0.0`.
fn is_ready(table: &str, port: u16) -> bool {
    listening_sockets(table).any(|(addr, p)| p == port && addr.is_unspecified())
}

/// Diagnosis when the container exited; `None` while it is still coming up.
fn exited(name: &UnitName) -> Option<HealthCheckError> {
    let c = inspect_container(name)?;
    match c.state.status.as_str() {
        // Still starting (or up): not an exit.
        "running" | "created" => None,
        _ => Some(HealthCheckError::Exited {
            exit_code: c.state.exit_code,
            uptime: uptime(&c.state.started_at, &c.state.finished_at),
        }),
    }
}

/// Work out why the wait timed out, preferring the most specific cause.
fn diagnose(name: &UnitName, container: &str, port: u16) -> HealthCheckError {
    // A crash in the last sleep window beats any port reasoning.
    if let Some(err) = exited(name) {
        return err;
    }

    match run_podman(&["exec", container, "cat", "/proc/net/tcp"]) {
        Ok(table) => {
            let found: Vec<String> = listening_sockets(&table)
                .map(|(addr, p)| format!("{addr}:{p}"))
                .collect();
            if found.is_empty() {
                HealthCheckError::NoPorts { expected: port }
            } else {
                HealthCheckError::WrongBinding {
                    expected: port,
                    found,
                }
            }
        }
        Err(_) => HealthCheckError::Timeout { expected: port },
    }
}

/// Yield `(local_addr, port)` for every TCP_LISTEN row in a `/proc/net/tcp`
/// table. `/proc/net/tcp` prints the address host-endian and the port as the
/// real value; our targets are little-endian, so `to_le_bytes` recovers the
/// dotted IP.
fn listening_sockets(table: &str) -> impl Iterator<Item = (Ipv4Addr, u16)> + '_ {
    table.lines().skip(1).filter_map(|line| {
        let mut fields = line.split_ascii_whitespace();

        // Line layout (after the header): `sl  local_addr:port rem_addr:port state …`
        let (_sl, local, _rem, state) =
            (fields.next(), fields.next()?, fields.next(), fields.next()?);
        if state != "0A" {
            return None;
        }

        let (addr_hex, port_hex) = local.split_once(':')?;
        let addr = u32::from_str_radix(addr_hex, 16).ok()?;
        let port = u16::from_str_radix(port_hex, 16).ok()?;
        Some((Ipv4Addr::from(addr.to_le_bytes()), port))
    })
}

/// Human-readable span between two RFC3339 stamps, or `"unknown"` when either
/// is unparseable (or `finished` precedes `started`, e.g. a zero-value stamp).
fn uptime(started: &str, finished: &str) -> String {
    let parse = |s: &str| DateTime::parse_from_rfc3339(s).ok();
    match (parse(started), parse(finished)) {
        (Some(start), Some(end)) => {
            let secs = (end - start).num_seconds();
            if secs < 0 {
                "unknown".to_string()
            } else if secs < 60 {
                format!("{secs}s")
            } else {
                format!("{}m{}s", secs / 60, secs % 60)
            }
        }
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode";

    fn table(rows: &[&str]) -> String {
        let mut out = String::from(HEADER);
        out.push('\n');
        for row in rows {
            out.push_str(row);
            out.push('\n');
        }
        out
    }

    fn sockets(table: &str) -> Vec<(Ipv4Addr, u16)> {
        listening_sockets(table).collect()
    }

    #[test]
    fn ready_on_wildcard_target_port() {
        // 0.0.0.0:8080 listening.
        let t = table(&[
            "   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(is_ready(&t, 8080));
    }

    #[test]
    fn not_ready_on_loopback_target_port() {
        // 127.0.0.1:8080 listening — reachable from inside the container only,
        // not by the reverse proxy. Must NOT be treated as healthy.
        let t = table(&[
            "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(!is_ready(&t, 8080));
        // It is still surfaced as a found socket for the WrongBinding message.
        assert_eq!(sockets(&t), vec![(Ipv4Addr::new(127, 0, 0, 1), 8080)]);
    }

    #[test]
    fn not_ready_on_established_socket_whose_remote_port_matches() {
        // local port 0xC000, remote port 0x1F90, state ESTABLISHED (01).
        let t = table(&[
            "   0: 0100007F:C000 0100007F:1F90 01 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(!is_ready(&t, 8080));
        assert!(sockets(&t).is_empty());
    }

    #[test]
    fn not_ready_on_different_port() {
        // 0.0.0.0:80 listening, but we want 8080.
        let t = table(&[
            "   0: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(!is_ready(&t, 8080));
        assert_eq!(sockets(&t), vec![(Ipv4Addr::UNSPECIFIED, 80)]);
    }

    #[test]
    fn no_sockets_for_empty_and_header_only() {
        assert!(sockets("").is_empty());
        assert!(sockets(HEADER).is_empty());
        assert!(!is_ready("", 8080));
        assert!(!is_ready(HEADER, 8080));
    }

    #[test]
    fn error_messages() {
        assert_eq!(
            HealthCheckError::Exited {
                exit_code: 137,
                uptime: "2s".to_string(),
            }
            .to_string(),
            "container exited with code 137 (ran 2s)"
        );
        assert_eq!(
            HealthCheckError::NoPorts { expected: 8080 }.to_string(),
            "container is running but not listening on any port (expected 8080 on 0.0.0.0)"
        );
        assert_eq!(
            HealthCheckError::WrongBinding {
                expected: 8080,
                found: vec!["127.0.0.1:8080".to_string(), "0.0.0.0:9000".to_string()],
            }
            .to_string(),
            "port 8080 not reachable on 0.0.0.0; found 127.0.0.1:8080, 0.0.0.0:9000"
        );
        assert_eq!(
            HealthCheckError::Timeout { expected: 8080 }.to_string(),
            "timed out waiting for port 8080 on 0.0.0.0"
        );
    }

    #[test]
    fn uptime_formats() {
        assert_eq!(
            uptime(
                "2026-05-20T10:11:12.000000000Z",
                "2026-05-20T10:11:14.000000000Z"
            ),
            "2s"
        );
        assert_eq!(
            uptime(
                "2026-05-20T10:11:12Z",
                "2026-05-20T10:12:15Z"
            ),
            "1m3s"
        );
        // Unparseable, or a zero-value FinishedAt (before StartedAt).
        assert_eq!(uptime("nope", "2026-05-20T10:11:14Z"), "unknown");
        assert_eq!(
            uptime("2026-05-20T10:11:12Z", "0001-01-01T00:00:00Z"),
            "unknown"
        );
    }
}
