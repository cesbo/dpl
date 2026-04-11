use std::{
    fs,
    path::{
        Path,
        PathBuf,
    },
    process::{
        Command,
        Stdio,
    },
};

use tracing::{
    error,
    info,
};

const SYSTEMD_DIR: &str = "/etc/systemd/system";

pub fn cleanup_app(name: &str) {
    let name = format!("dpl--{name}.service");
    let path = Path::new(SYSTEMD_DIR).join(&name);

    run_systemctl(&["disable", "--now", &name]);
    remove_unit(&name, &path);
    run_systemctl(&["daemon-reload"]);
}

/// Stop and remove all stale timer-related unit files for the given entity.
pub fn cleanup_timers(name: &str) {
    let prefix = format!("dpl--{name}--");
    let dir = Path::new(SYSTEMD_DIR);

    let mut timers: Vec<(String, PathBuf)> = Vec::new();
    let mut services: Vec<(String, PathBuf)> = Vec::new();

    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            error!("failed to read directory {}: {}", dir.display(), err);
            return;
        }
    };

    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(_) => continue,
        };
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !file_name.starts_with(&prefix) {
            continue;
        } else if file_name.ends_with(".timer") {
            timers.push((file_name.to_owned(), path));
        } else if file_name.ends_with(".service") {
            services.push((file_name.to_owned(), path));
        }
    }

    if timers.is_empty() && services.is_empty() {
        return;
    }

    for (name, path) in &timers {
        run_systemctl(&["disable", "--now", name]);
        remove_unit(name, path);
    }

    for (name, path) in &services {
        run_systemctl(&["stop", name]);
        remove_unit(name, path);
    }

    run_systemctl(&["daemon-reload"]);
}

fn remove_unit(name: &str, path: &Path) {
    match fs::remove_file(path) {
        Ok(_) => {
            info!("removed stale unit file {}", name);
        }
        Err(err) => {
            error!("failed to remove {}: {}", path.display(), err);
        }
    }
}

/// Run `systemctl <args...>`
fn run_systemctl(args: &[&str]) {
    let result = Command::new("systemctl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .status();

    match result {
        Ok(status) if status.success() => {
            info!("systemctl {} ok", args.join(" "));
        }
        Ok(status) => {
            error!("systemctl {} exited with {}", args.join(" "), status);
        }
        Err(err) => {
            error!("failed to spawn systemctl {}: {}", args.join(" "), err);
        }
    }
}
