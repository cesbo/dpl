use std::{
    io,
    net::{
        IpAddr,
        Ipv4Addr,
        Ipv6Addr,
    },
    thread::sleep,
    time::{
        Duration,
        Instant,
    },
};

use chrono::DateTime;
use thiserror::Error;

use crate::{
    config::UnitName,
    podman::{
        ContainerStatus,
        run_podman_within,
        try_inspect_container,
    },
};

/// Total wall-clock budget for one readiness wait. A slow-but-healthy boot (a
/// JVM, an app running migrations) takes tens of seconds; this stays below the
/// supervisor's `STARTUP_GRACE` so the deploy reaches a verdict before serve
/// stops treating the container as still starting.
pub const BUDGET: Duration = Duration::from_secs(90);

/// Pacing between probes - not a budget.
const INTERVAL: Duration = Duration::from_millis(800);

/// Cap on a single probe, so a wedged `podman exec` costs one attempt instead
/// of the whole deploy.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often a long wait restates what it is waiting for.
const HEARTBEAT: Duration = Duration::from_secs(10);

/// Consecutive `running` observations that mark a port-less unit as up.
const SETTLE: usize = 3;

#[derive(Debug, Error)]
pub enum HealthCheckError {
    #[error("container exited with code {exit_code} (ran {uptime})")]
    Exited { exit_code: i32, uptime: String },

    #[error(
        "waited {waited}: container is running but not listening on any port (expected {expected})"
    )]
    NoPorts { expected: u16, waited: String },

    #[error("waited {waited}: port {expected} not reachable; found {}", .found.join(", "))]
    WrongBinding {
        expected: u16,
        found: Vec<String>,
        waited: String,
    },

    #[error("waited {waited}: the container was never created; make sure dpl serve is running")]
    NotStarted { waited: String },

    /// The probe itself never got an answer - podman, not the unit, is the
    /// suspect. Keeps a wedged host from being reported as a broken app.
    /// The cause comes from the `#[source]` chain, so it is not repeated here.
    #[error("waited {waited}: readiness probe for container {container} failed")]
    Probe {
        container: String,
        waited: String,
        #[source]
        source: io::Error,
    },
}

/// The probes and pacing of one readiness wait. Injected so the wait loops are
/// testable without podman.
struct Probes<'a> {
    /// The container's concatenated `/proc/net/{tcp,tcp6}` tables.
    table: &'a mut dyn FnMut() -> io::Result<String>,
    /// The container's state; `Ok(None)` while it does not exist yet.
    state: &'a mut dyn FnMut() -> io::Result<Option<ContainerStatus>>,
    budget: Duration,
    interval: Duration,
}

/// Wait until the unit is up, for at most [`BUDGET`]. With a `port`, up means a
/// listening TCP socket on it bound to a wildcard address (`0.0.0.0` or `::`).
/// Without one - a service that listens on nothing - up means the container
/// started and stayed running, the strongest signal available.
pub fn check(name: &UnitName, port: Option<u16>) -> Result<(), HealthCheckError> {
    let container = name.scoped_unit_name();
    let mut table = || tcp_tables(&container);
    let mut state =
        || try_inspect_container(name, false, PROBE_TIMEOUT).map(|c| c.map(|c| c.state));

    let mut probes = Probes {
        table: &mut table,
        state: &mut state,
        budget: BUDGET,
        interval: INTERVAL,
    };

    match port {
        Some(port) => check_port(&container, port, &mut probes),
        None => check_running(&container, &mut probes),
    }
}

/// One-line statement of what readiness means for a unit, shown verbatim by the
/// deploy console and `dpl inspect` so a long wait is never unexplained.
pub fn criterion(port: Option<u16>) -> String {
    match port {
        Some(port) => format!(
            "a listening socket on 0.0.0.0:{port} or [::]:{port} \
             (probe: podman exec cat /proc/net/tcp)"
        ),
        None => format!(
            "{SETTLE} consecutive `running` observations \
             (probe: podman container inspect)"
        ),
    }
}

fn check_port(container: &str, port: u16, p: &mut Probes<'_>) -> Result<(), HealthCheckError> {
    let started = Instant::now();
    let deadline = started + p.budget;
    let mut heartbeat = started;
    let mut last_table: Option<String> = None;
    let mut last_err: Option<io::Error>;

    loop {
        sleep(p.interval);

        match (p.table)() {
            Ok(table) if is_ready(&table, port) => return Ok(()),
            Ok(table) => {
                last_table = Some(table);
                last_err = None;
            }
            Err(err) => {
                // A failing probe can simply mean the container died; report
                // that first, and keep the error for the give-up message.
                if let Some(exit) = exit_verdict(p) {
                    return Err(exit);
                }
                last_err = Some(err);
            }
        }

        if Instant::now() >= deadline {
            break;
        }

        if heartbeat.elapsed() >= HEARTBEAT {
            heartbeat = Instant::now();
            report_progress(
                container,
                Some(port),
                started.elapsed(),
                p.budget,
                last_err.as_ref(),
            );
        }
    }

    let waited = crate::log::fmt_duration(started.elapsed());
    if let Some(verdict) = container_verdict(p, &waited) {
        return Err(verdict);
    }

    Err(match (last_err, last_table) {
        // A probe that never answered blames podman, not the unit.
        (Some(source), _) => HealthCheckError::Probe {
            container: container.to_string(),
            waited,
            source,
        },
        // Classified from the table already in hand - no extra podman call.
        (None, Some(table)) => classify_binding_err(&table, port, &waited),
        (None, None) => HealthCheckError::Probe {
            container: container.to_string(),
            waited,
            source: io::Error::other("no readiness probe completed"),
        },
    })
}

fn check_running(container: &str, p: &mut Probes<'_>) -> Result<(), HealthCheckError> {
    let started = Instant::now();
    let deadline = started + p.budget;
    let mut heartbeat = started;
    let mut running = 0;
    let mut last_err: Option<io::Error>;

    loop {
        sleep(p.interval);

        match (p.state)() {
            Ok(Some(status)) => {
                if let Some(err) = exit_error(&status) {
                    return Err(err);
                }
                // SETTLE counts consecutive observations, so anything else resets.
                running = if status.status == "running" {
                    running + 1
                } else {
                    0
                };
                if running == SETTLE {
                    return Ok(());
                }
                last_err = None;
            }
            // Absent means serve has not created the container yet.
            Ok(None) => {
                running = 0;
                last_err = None;
            }
            Err(err) => {
                running = 0;
                last_err = Some(err);
            }
        }

        if Instant::now() >= deadline {
            break;
        }

        if heartbeat.elapsed() >= HEARTBEAT {
            heartbeat = Instant::now();
            report_progress(
                container,
                None,
                started.elapsed(),
                p.budget,
                last_err.as_ref(),
            );
        }
    }

    let waited = crate::log::fmt_duration(started.elapsed());
    Err(match last_err {
        Some(source) => HealthCheckError::Probe {
            container: container.to_string(),
            waited,
            source,
        },
        None => HealthCheckError::NotStarted { waited },
    })
}

/// Restate the wait on the deploy console, so a long one is never opaque.
fn report_progress(
    container: &str,
    port: Option<u16>,
    elapsed: Duration,
    budget: Duration,
    last_err: Option<&io::Error>,
) {
    let detail = match last_err {
        Some(err) => format!(" - last probe error: {err}"),
        None => String::new(),
    };

    crate::log::progress(format!(
        "waiting for {container}: {} - {} of {}{detail}",
        criterion(port),
        crate::log::fmt_duration(elapsed),
        crate::log::fmt_duration(budget),
    ));
}

fn tcp_tables(container: &str) -> io::Result<String> {
    run_podman_within(
        &[
            "exec",
            container,
            "sh",
            "-c",
            "cat /proc/net/tcp /proc/net/tcp6 2>/dev/null || true",
        ],
        PROBE_TIMEOUT,
    )
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

/// The container's terminal-state error, if it already reached one.
fn exit_verdict(p: &mut Probes<'_>) -> Option<HealthCheckError> {
    (p.state)().ok().flatten().as_ref().and_then(exit_error)
}

/// One final state read once the budget is spent: the verdict the container
/// itself supports (it exited, or it was never created), if any.
fn container_verdict(p: &mut Probes<'_>, waited: &str) -> Option<HealthCheckError> {
    match (p.state)().ok().flatten() {
        Some(status) => exit_error(&status),
        None => Some(HealthCheckError::NotStarted {
            waited: waited.to_string(),
        }),
    }
}

/// The error for a container that reached a terminal state; `None` while it is
/// still `created` or `running`.
fn exit_error(state: &ContainerStatus) -> Option<HealthCheckError> {
    match state.status.as_str() {
        // Still starting (or up): not an exit.
        "running" | "created" => None,
        _ => Some(HealthCheckError::Exited {
            exit_code: state.exit_code,
            uptime: uptime(&state.started_at, &state.finished_at),
        }),
    }
}

/// Classify why a reachable container failed the wait.
fn classify_binding_err(table: &str, port: u16, waited: &str) -> HealthCheckError {
    let found: Vec<String> = listening_sockets(table)
        .map(|(addr, p)| format!("{addr}:{p}"))
        .collect();

    if found.is_empty() {
        HealthCheckError::NoPorts {
            expected: port,
            waited: waited.to_string(),
        }
    } else {
        HealthCheckError::WrongBinding {
            expected: port,
            found,
            waited: waited.to_string(),
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
    use std::cell::Cell;

    use super::*;

    const HEADER: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode";

    /// `0.0.0.0:8080` LISTEN.
    const WILDCARD_8080: &str = "   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0";

    /// `127.0.0.1:8080` LISTEN - a listener the reverse proxy cannot reach.
    const LOOPBACK_8080: &str = "   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0";

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

    fn status(state: &str, exit_code: i32) -> ContainerStatus {
        ContainerStatus {
            status: state.to_string(),
            started_at: "2026-05-20T10:11:12Z".to_string(),
            finished_at: "2026-05-20T10:11:14Z".to_string(),
            exit_code,
        }
    }

    /// Pacing for the loop tests: a handful of iterations, no real waiting.
    fn fast(budget: Duration) -> (Duration, Duration) {
        (budget, Duration::from_millis(2))
    }

    #[test]
    fn ready_when_the_wildcard_socket_appears() {
        let calls = Cell::new(0);
        let (budget, interval) = fast(Duration::from_secs(5));
        let result = {
            let mut probe = || {
                calls.set(calls.get() + 1);
                Ok(if calls.get() >= 3 {
                    table(&[WILDCARD_8080])
                } else {
                    table(&[])
                })
            };
            let mut state = || Ok(Some(status("running", 0)));
            check_port(
                "dpl--web",
                8080,
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        assert!(result.is_ok(), "{:?}", result.err());
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn classifies_from_the_last_table_without_an_extra_probe() {
        let state_calls = Cell::new(0);
        let (budget, interval) = fast(Duration::from_millis(30));
        let result = {
            let mut probe = || Ok(table(&[LOOPBACK_8080]));
            let mut state = || {
                state_calls.set(state_calls.get() + 1);
                Ok(Some(status("running", 0)))
            };
            check_port(
                "dpl--web",
                8080,
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        let Err(HealthCheckError::WrongBinding {
            expected, found, ..
        }) = result
        else {
            panic!("expected WrongBinding, got {result:?}");
        };
        assert_eq!(expected, 8080);
        assert_eq!(found, vec!["127.0.0.1:8080".to_string()]);
        // Only the single give-up state read - the socket table came from the loop.
        assert_eq!(state_calls.get(), 1);
    }

    #[test]
    fn probe_failure_blames_podman_not_the_app() {
        let (budget, interval) = fast(Duration::from_millis(30));
        let result = {
            let mut probe = || {
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "podman exec dpl--web did not return within 5s",
                ))
            };
            let mut state = || Ok(Some(status("running", 0)));
            check_port(
                "dpl--web",
                8080,
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        let Err(err @ HealthCheckError::Probe { .. }) = result else {
            panic!("expected Probe, got {result:?}");
        };
        let msg = err.to_string();
        assert!(msg.contains("readiness probe"), "{msg}");
        assert!(msg.contains("dpl--web"), "{msg}");
    }

    #[test]
    fn container_exit_during_the_wait_is_reported() {
        let (budget, interval) = fast(Duration::from_secs(5));
        let result = {
            let mut probe = || Err(io::Error::other("no such container"));
            let mut state = || Ok(Some(status("exited", 1)));
            check_port(
                "dpl--web",
                8080,
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        assert!(
            matches!(result, Err(HealthCheckError::Exited { exit_code: 1, .. })),
            "{result:?}"
        );
    }

    #[test]
    fn absent_container_reports_not_started() {
        let (budget, interval) = fast(Duration::from_millis(30));
        let result = {
            let mut probe = || Err(io::Error::other("no such container"));
            let mut state = || Ok(None);
            check_port(
                "dpl--web",
                8080,
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        let Err(err @ HealthCheckError::NotStarted { .. }) = result else {
            panic!("expected NotStarted, got {result:?}");
        };
        assert!(err.to_string().contains("dpl serve"), "{err}");
    }

    #[test]
    fn port_wait_respects_the_budget() {
        let started = Instant::now();
        let (budget, interval) = fast(Duration::from_millis(50));
        let result = {
            let mut probe = || Ok(table(&[]));
            let mut state = || Ok(Some(status("running", 0)));
            check_port(
                "dpl--web",
                8080,
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        assert!(
            matches!(result, Err(HealthCheckError::NoPorts { .. })),
            "{result:?}"
        );
        // The whole point of the deadline: it gives up on time.
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "overshot budget"
        );
    }

    #[test]
    fn settle_requires_consecutive_running() {
        let calls = Cell::new(0);
        let (budget, interval) = fast(Duration::from_secs(5));
        let result = {
            let states = [
                "created", "running", "created", "running", "running", "running",
            ];
            let mut probe = || Ok(table(&[]));
            let mut state = || {
                let index = calls.get();
                calls.set(index + 1);
                Ok(Some(status(states[index.min(states.len() - 1)], 0)))
            };
            check_running(
                "dpl--worker",
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        assert!(result.is_ok(), "{:?}", result.err());
        // Six observations: the `created` at index 2 must reset the counter.
        assert_eq!(calls.get(), 6);
    }

    #[test]
    fn running_wait_reports_probe_error_when_podman_never_answers() {
        let (budget, interval) = fast(Duration::from_millis(30));
        let result = {
            let mut probe = || Ok(table(&[]));
            let mut state = || Err(io::Error::new(io::ErrorKind::TimedOut, "wedged"));
            check_running(
                "dpl--worker",
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        assert!(
            matches!(result, Err(HealthCheckError::Probe { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn not_started_when_the_container_never_appears() {
        let (budget, interval) = fast(Duration::from_millis(30));
        let result = {
            let mut probe = || Ok(table(&[]));
            let mut state = || Ok(None);
            check_running(
                "dpl--worker",
                &mut Probes {
                    table: &mut probe,
                    state: &mut state,
                    budget,
                    interval,
                },
            )
        };

        assert!(
            matches!(result, Err(HealthCheckError::NotStarted { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn criterion_states_the_probe() {
        let port = criterion(Some(8080));
        assert!(port.contains("0.0.0.0:8080"), "{port}");
        assert!(port.contains("[::]:8080"), "{port}");
        assert!(port.contains("/proc/net/tcp"), "{port}");

        let portless = criterion(None);
        assert!(portless.contains("3 consecutive"), "{portless}");
        assert!(portless.contains("podman container inspect"), "{portless}");
    }

    #[test]
    fn ready_on_wildcard_target_port() {
        // 0.0.0.0:8080 listening.
        let t = table(&[WILDCARD_8080]);
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
        let mut t = table(&[LOOPBACK_8080]);
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
        let t = table(&[LOOPBACK_8080]);
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
            classify_binding_err(HEADER, 8080, "90s"),
            HealthCheckError::NoPorts { expected: 8080, .. }
        ));
    }

    #[test]
    fn classify_loopback_is_wrong_binding_with_found() {
        // 127.0.0.1:8080 is a listener, just not reachable off-host: WrongBinding,
        // and the loopback socket is surfaced for the operator.
        let t = table(&[LOOPBACK_8080]);
        let HealthCheckError::WrongBinding {
            expected,
            found,
            waited,
        } = classify_binding_err(&t, 8080, "90s")
        else {
            panic!("expected WrongBinding");
        };
        assert_eq!(expected, 8080);
        assert_eq!(found, vec!["127.0.0.1:8080".to_string()]);
        assert_eq!(waited, "90s");
    }

    #[test]
    fn classify_wrong_port_is_wrong_binding_with_found() {
        // Listening on the wildcard but the wrong port (0.0.0.0:80, want 8080).
        let t = table(&[
            "   0: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        let HealthCheckError::WrongBinding {
            expected, found, ..
        } = classify_binding_err(&t, 8080, "90s")
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
        let HealthCheckError::WrongBinding { found, .. } = classify_binding_err(&t, 8080, "90s")
        else {
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
