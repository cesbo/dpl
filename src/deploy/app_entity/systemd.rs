use std::{
    fs,
    io,
    path::Path,
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

fn remove_timer_unit(systemd_dir: &Path, unit: &str) -> bool {
    let path = systemd_dir.join(unit);
    match fs::remove_file(&path) {
        Ok(_) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            info!("timer unit {unit} not found");
            false
        }
        Err(err) => {
            error!("failed to remove timer unit {unit}: {err}");
            false
        }
    }
}

fn remove_timer(systemd_dir: &Path, prefix: &str) {
    let timer_unit = format!("{prefix}.timer");
    let _ = run_systemctl(&["disable", "--now", &timer_unit]);
    let removed = remove_timer_unit(systemd_dir, &timer_unit);

    let timer_service = format!("{prefix}.service");
    let _ = run_systemctl(&["stop", &timer_service]);
    remove_timer_unit(systemd_dir, &timer_service);

    if removed {
        info!("timer {prefix} removed");
    }
}

fn copy_timer_unit(artifacts_dir: &Path, systemd_dir: &Path, unit: &str) -> bool {
    let src = artifacts_dir.join(unit);
    let dst = systemd_dir.join(unit);
    match fs::copy(&src, &dst) {
        Ok(_) => true,
        Err(err) => {
            error!("failed to copy timer service {unit}: {err}");
            false
        }
    }
}

fn copy_timer(artifacts_dir: &Path, systemd_dir: &Path, prefix: &str) -> bool {
    let timer_service = format!("{prefix}.service");
    if !copy_timer_unit(artifacts_dir, systemd_dir, &timer_service) {
        return false;
    }

    let timer_unit = format!("{prefix}.timer");
    if !copy_timer_unit(artifacts_dir, systemd_dir, &timer_unit) {
        remove_timer_unit(systemd_dir, &timer_service);
        return false;
    }

    true
}

fn list_timers(dir: &Path, prefix: &str) -> Vec<String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => {
            error!("failed to read directory {}: {err}", dir.display());
            return Vec::new();
        }
    };

    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|n| n.to_str())
                .filter(|n| n.starts_with(prefix))
                .and_then(|n| n.strip_suffix(".timer"))
                .map(|n| n.to_owned())
        })
        .collect()
}

fn install_timers(deploy_dir: &Path) {
    let systemd_dir = Path::new(SYSTEMD_DIR);
    let artifacts_dir = deploy_dir.join("artifacts");

    let mut timers: Vec<String> = list_timers(&artifacts_dir, "dpl--");
    timers.retain(|p: &String| copy_timer(&artifacts_dir, systemd_dir, p));

    if timers.is_empty() {
        return;
    }

    reload_systemd();

    for prefix in &timers {
        let unit = format!("{prefix}.timer");
        match run_systemctl(&["enable", "--now", &unit]) {
            Ok(_) => {
                info!("timer {unit} installed")
            }
            Err(err) => {
                remove_timer(systemd_dir, prefix);
                error!("failed to install timer {unit}: {err}");
            }
        }
    }
}

/// Stop and remove all stale timer-related unit files for the given entity.
fn uninstall_timers(name: &str) {
    let prefix = format!("dpl--{name}--");
    let systemd_dir = Path::new(SYSTEMD_DIR);

    let timers = list_timers(systemd_dir, &prefix);

    if timers.is_empty() {
        return;
    }

    for prefix in &timers {
        remove_timer(systemd_dir, prefix);
    }

    reload_systemd();
}

pub fn install_app(name: &str, artifacts_dir: &Path) -> io::Result<()> {
    let unit = format!("dpl--{name}.service");
    let src = artifacts_dir.join(&unit);
    let dst = Path::new(SYSTEMD_DIR).join(&unit);

    fs::copy(&src, &dst)?;

    reload_systemd();

    if let Err(err) = run_systemctl(&["enable", "--now", &unit]) {
        let _ = fs::remove_file(&dst);
        Err(err)
    } else {
        Ok(())
    }
}

fn uninstall_app(name: &str) {
    let systemd_dir = Path::new(SYSTEMD_DIR);
    let prefix = format!("dpl--{name}");

    let app_service = format!("{prefix}.service");
    let app_service_path = systemd_dir.join(&app_service);
    let _ = run_systemctl(&["disable", "--now", &app_service]);
    if let Err(err) = fs::remove_file(&app_service_path) {
        if err.kind() == io::ErrorKind::NotFound {
            info!("app service {prefix} not found");
        } else {
            error!("failed to remove app service {prefix}: {err}");
        }
    } else {
        info!("app service {prefix} removed");
        reload_systemd();
    }
}

fn reload_systemd() {
    if let Err(err) = run_systemctl(&["daemon-reload"]) {
        error!("reload systemd: {}", err);
    }
}

fn run_systemctl(args: &[&str]) -> io::Result<()> {
    let status = Command::new("systemctl")
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("systemctl exited with {}", status),
        ))
    }
}
