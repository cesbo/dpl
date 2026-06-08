use std::{
    io,
    net::{
        IpAddr,
        Ipv4Addr,
        Ipv6Addr,
    },
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

#[derive(Debug, Error)]
pub enum HealthCheckError {
    #[error("container exited with code {exit_code} (ran {uptime})")]
    Exited { exit_code: i32, uptime: String },

    #[error("container is running but not listening on any port (expected {expected})")]
    NoPorts { expected: u16 },

    #[error("port {expected} not reachable; found {}", .found.join(", "))]
    WrongBinding { expected: u16, found: Vec<String> },

    #[error("timed out waiting for port {expected}; make sure dpl serve is running")]
    Timeout { expected: u16 },
}

/// Wait until the container has a listening TCP socket on `port` bound to a
/// wildcard address (`0.0.0.0` or `::`).
pub fn check(name: &UnitName, port: u16) -> Result<(), HealthCheckError> {
    let container = name.scoped_unit_name();

    for _ in 0 .. ATTEMPTS {
        sleep(INTERVAL);

        match tcp_tables(&container) {
            Ok(table) if is_ready(&table, port) => return Ok(()),
            Ok(_) => {}
            Err(_) => exited(name)?,
        }
    }

    exited(name)?;

    let err = match tcp_tables(&container) {
        Ok(table) => classify_binding_err(&table, port),
        Err(_) => HealthCheckError::Timeout { expected: port },
    };

    Err(err)
}

fn tcp_tables(container: &str) -> io::Result<String> {
    run_podman(&[
        "exec",
        container,
        "sh",
        "-c",
        "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null || true",
    ])
}

fn is_ready(table: &str, port: u16) -> bool {
    listening_sockets(table).any(|(addr, p)| p == port && is_wildcard(addr))
}

fn is_wildcard(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(a) => a.is_unspecified(),
        IpAddr::V6(a) => {
            a.is_unspecified() || a.to_ipv4_mapped().is_some_and(|v4| v4.is_unspecified())
        }
    }
}

fn exited(name: &UnitName) -> Result<(), HealthCheckError> {
    let Some(c) = inspect_container(name, false) else {
        return Ok(());
    };

    match c.state.status.as_str() {
        // Still starting (or up): not an exit.
        "running" | "created" => Ok(()),
        _ => Err(HealthCheckError::Exited {
            exit_code: c.state.exit_code,
            uptime: uptime(&c.state.started_at, &c.state.finished_at),
        }),
    }
}

/// Classify why a reachable container failed the wait.
fn classify_binding_err(table: &str, port: u16) -> HealthCheckError {
    let found: Vec<String> = listening_sockets(table)
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

/// Yield `(local_addr, port)` for every TCP_LISTEN row across one or more
/// concatenated `/proc/net/{tcp,tcp6}` tables.
/// Header lines are dropped by the `state != "0A"` filter.
fn listening_sockets(table: &str) -> impl Iterator<Item = (IpAddr, u16)> + '_ {
    table.lines().filter_map(|line| {
        let mut fields = line.split_ascii_whitespace();

        // Line layout: `sl  local_addr:port rem_addr:port state …`
        let local = fields.nth(1)?;
        let state = fields.nth(1)?;

        if state != "0A" {
            return None;
        }

        let (addr_hex, port_hex) = local.split_once(':')?;
        let addr = parse_addr(addr_hex)?;
        let port = u16::from_str_radix(port_hex, 16).ok()?;
        Some((addr, port))
    })
}

/// Parse a `/proc/net/{tcp,tcp6}` hex local-address.
fn parse_addr(hex: &str) -> Option<IpAddr> {
    match hex.len() {
        8 => {
            let raw = u32::from_str_radix(hex, 16).ok()?;
            Some(IpAddr::V4(Ipv4Addr::from(raw.to_le_bytes())))
        }
        32 => {
            let mut octets = [0u8; 16];
            for (word, chunk) in octets.chunks_exact_mut(4).enumerate() {
                let part = hex.get(word * 8 .. word * 8 + 8)?;
                let raw = u32::from_str_radix(part, 16).ok()?;
                chunk.copy_from_slice(&raw.to_le_bytes());
            }
            Some(IpAddr::V6(Ipv6Addr::from(octets)))
        }
        _ => None,
    }
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

    fn sockets(table: &str) -> Vec<(IpAddr, u16)> {
        listening_sockets(table).collect()
    }

    fn v4(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
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
    fn ready_on_ipv6_wildcard() {
        // [::]:8080 listening (32-hex all-zero address) - busybox httpd and many
        // servers bind the IPv6 wildcard, which also accepts IPv4. Must be ready.
        let t = table(&[
            "   0: 00000000000000000000000000000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(is_ready(&t, 8080));
        assert_eq!(sockets(&t), vec![(IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8080)]);
    }

    #[test]
    fn ready_on_ipv4_mapped_wildcard() {
        // [::ffff:0.0.0.0]:8080 - an IPv4-mapped wildcard, also reachable. The
        // `ffff` group sits in the third 32-bit word, as the kernel prints it.
        let t = table(&[
            "   0: 0000000000000000FFFF000000000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(is_ready(&t, 8080));
    }

    #[test]
    fn not_ready_on_ipv6_loopback() {
        // [::1]:8080 - loopback only, not reachable by the reverse proxy.
        let t = table(&[
            "   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(!is_ready(&t, 8080));
        assert_eq!(sockets(&t), vec![(IpAddr::V6(Ipv6Addr::LOCALHOST), 8080)]);
    }

    #[test]
    fn ready_finds_ipv6_wildcard_among_concatenated_tables() {
        // Mirrors `cat /proc/net/tcp /proc/net/tcp6`: an IPv4 loopback row, then
        // the tcp6 header, then the IPv6 wildcard. The interior header must be
        // ignored and the wildcard found.
        let mut t = table(&[
            "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        t.push_str(HEADER);
        t.push('\n');
        t.push_str("   0: 00000000000000000000000000000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0\n");
        assert!(is_ready(&t, 8080));
        assert_eq!(
            sockets(&t),
            vec![
                (v4(127, 0, 0, 1), 8080),
                (IpAddr::V6(Ipv6Addr::UNSPECIFIED), 8080),
            ]
        );
    }

    #[test]
    fn not_ready_on_loopback_target_port() {
        // 127.0.0.1:8080 listening - reachable from inside the container only,
        // not by the reverse proxy. Must NOT be treated as healthy.
        let t = table(&[
            "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(!is_ready(&t, 8080));
        // It is still surfaced as a found socket for the WrongBinding message.
        assert_eq!(sockets(&t), vec![(v4(127, 0, 0, 1), 8080)]);
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
        assert_eq!(sockets(&t), vec![(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 80)]);
    }

    #[test]
    fn no_sockets_for_empty_and_header_only() {
        assert!(sockets("").is_empty());
        assert!(sockets(HEADER).is_empty());
        assert!(!is_ready("", 8080));
        assert!(!is_ready(HEADER, 8080));
    }

    #[test]
    fn classify_no_listeners_is_no_ports() {
        // Nothing listening (header only): the container is up but bound nothing.
        assert!(matches!(
            classify_binding_err(HEADER, 8080),
            HealthCheckError::NoPorts { expected: 8080 }
        ));
    }

    #[test]
    fn classify_loopback_is_wrong_binding_with_found() {
        // 127.0.0.1:8080 is a listener, just not reachable off-host: WrongBinding,
        // and the loopback socket is surfaced for the operator.
        let t = table(&[
            "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        let HealthCheckError::WrongBinding { expected, found } = classify_binding_err(&t, 8080)
        else {
            panic!("expected WrongBinding");
        };
        assert_eq!(expected, 8080);
        assert_eq!(found, vec!["127.0.0.1:8080".to_string()]);
    }

    #[test]
    fn classify_wrong_port_is_wrong_binding_with_found() {
        // Listening on the wildcard but the wrong port (0.0.0.0:80, want 8080).
        let t = table(&[
            "   0: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        let HealthCheckError::WrongBinding { expected, found } = classify_binding_err(&t, 8080)
        else {
            panic!("expected WrongBinding");
        };
        assert_eq!(expected, 8080);
        assert_eq!(found, vec!["0.0.0.0:80".to_string()]);
    }

    #[test]
    fn classify_ipv6_loopback_is_wrong_binding() {
        // [::1]:8080 - a listener, but loopback-only: WrongBinding, surfaced as
        // the bracketed IPv6 form.
        let t = table(&[
            "   0: 00000000000000000000000001000000:1F90 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        let HealthCheckError::WrongBinding { found, .. } = classify_binding_err(&t, 8080) else {
            panic!("expected WrongBinding");
        };
        assert_eq!(found, vec!["::1:8080".to_string()]);
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
            uptime("2026-05-20T10:11:12Z", "2026-05-20T10:12:15Z"),
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
