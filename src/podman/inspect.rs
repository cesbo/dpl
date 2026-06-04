use chrono::{
    DateTime,
    Utc,
};

use crate::{
    config::UnitName,
    log::{
        error_mark,
        fmt_ago,
        fmt_bytes,
        print_field,
        success_mark,
    },
    podman::{
        container_stats,
        image_size,
        inspect_container,
    },
};

/// Print live container/runtime detail.
pub fn print_container_state(name: &UnitName) {
    let Some(c) = inspect_container(name, true) else {
        print_field("Container", format!("{} unavailable", error_mark()));
        return;
    };

    let running = c.state.status == "running";
    let mark = if running {
        success_mark()
    } else {
        error_mark()
    };
    print_field("Container", format!("{mark} {}", c.state.status));

    if running {
        if let Ok(started) = DateTime::parse_from_rfc3339(&c.state.started_at) {
            print_field("Uptime", fmt_ago(&Utc::now(), &started.with_timezone(&Utc)));
        }
        if let Some(s) = container_stats(name) {
            print_field("CPU", s.cpu_perc);
            print_field("Memory", format!("{} ({})", s.mem_usage, s.mem_perc));
            if !s.net_io.is_empty() && s.net_io != "-- / --" {
                print_field("Network RX/TX", s.net_io);
            }
        }
    }

    print_field("Restarts", c.restart_count);
    print_field("Image", &c.image_name);
    if let Some(size) = image_size(&c.image_name) {
        print_field("Image size", fmt_bytes(size));
    }

    // Writable layer: how much the container has grown on top of the image.
    if let Some(rw) = c.size_rw.filter(|&rw| rw >= 0) {
        print_field("Writable layer", fmt_bytes(rw as u64));
    }

    if c.state.exit_code != 0 {
        print_field("Exit code", console::style(c.state.exit_code).red());
    }
}
