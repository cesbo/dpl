use std::{
    io,
    thread::sleep,
    time::Duration,
};

use crate::{
    config::ResourceName,
    podman::run_podman,
};

const ATTEMPTS: usize = 30;
const INTERVAL: Duration = Duration::from_millis(800);

/// Wait until the container has a listening TCP socket on `port`.
pub fn check(name: &ResourceName, port: u16) -> io::Result<()> {
    let container = name.scoped_unit_name();
    let port_hex = format!("{port:04X}");

    for _ in 0 .. ATTEMPTS {
        sleep(INTERVAL);

        if let Ok(table) = run_podman(&["exec", &container, "cat", "/proc/net/tcp"])
            && has_listening_port(&table, &port_hex)
        {
            return Ok(());
        }
    }

    Err(io::Error::other("timed out".to_string()))
}

/// Scan a `/proc/net/tcp` table for a listening socket on `port_hex`.
fn has_listening_port(table: &str, port_hex: &str) -> bool {
    table.lines().skip(1).any(|line| {
        let mut fields = line.split_ascii_whitespace();

        // Line layout (after the header): `sl  local_addr:port rem_addr:port state …`
        // Accept a row when `state == 0A` (TCP_LISTEN) and the local port matches.
        let (_sl, local, _rem, state) =
            (fields.next(), fields.next(), fields.next(), fields.next());
        match (local, state) {
            (Some(local), Some("0A")) => local.split(':').nth(1) == Some(port_hex),
            _ => false,
        }
    })
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

    #[test]
    fn matches_listening_socket_on_target_port() {
        let t = table(&[
            "   0: 00000000:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(has_listening_port(&t, "1F90"));
    }

    #[test]
    fn ignores_established_socket_whose_remote_port_matches() {
        // local port 0xC000, remote port 0x1F90 — old grep would have matched
        // ":1F90 " and falsely reported the port open.
        let t = table(&[
            "   0: 0100007F:C000 0100007F:1F90 01 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(!has_listening_port(&t, "1F90"));
    }

    #[test]
    fn ignores_listening_socket_on_different_port() {
        let t = table(&[
            "   0: 00000000:0050 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 1234 1 0000000000000000 100 0 0 10 0",
        ]);
        assert!(!has_listening_port(&t, "1F90"));
    }

    #[test]
    fn empty_and_header_only_inputs() {
        assert!(!has_listening_port("", "1F90"));
        assert!(!has_listening_port(HEADER, "1F90"));
    }
}
