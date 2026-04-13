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

fn remove_app(prefix: &str) {
    let dir = Path::new(SYSTEMD_DIR);

    let app_service = format!("{}.service", prefix);
    let app_service_path = dir.join(&app_service);
    let removed = run_systemctl(&["disable", "--now", &app_service]).is_ok();
    if let Err(err) = fs::remove_file(&app_service_path) {
        if err.kind() != io::ErrorKind::NotFound {
            error!("failed to remove app service {}: {}", prefix, err);
        }
    }

    if removed {
        info!("app service {} removed", prefix);
    }
}

fn remove_timer(prefix: &str) {
    let dir = Path::new(SYSTEMD_DIR);

    let timer_unit = format!("{}.timer", prefix);
    let timer_unit_path = dir.join(&timer_unit);
    let removed = run_systemctl(&["disable", "--now", &timer_unit]).is_ok();
    if let Err(err) = fs::remove_file(&timer_unit_path) {
        if err.kind() != io::ErrorKind::NotFound {
            error!("failed to remove timer unit {}: {}", prefix, err);
        }
    }

    let timer_service = format!("{}.service", prefix);
    let timer_service_path = dir.join(&timer_service);
    let _ = run_systemctl(&["stop", &timer_service]);
    if let Err(err) = fs::remove_file(&timer_service_path) {
        if err.kind() != io::ErrorKind::NotFound {
            error!("failed to remove timer service {}: {}", prefix, err);
        }
    }

    if removed {
        info!("timer {} removed", prefix);
    }
}

fn copy_timer(prefix: &str, artifacts_dir: &Path) -> bool {
    let dst_dir = Path::new(SYSTEMD_DIR);

    let timer_service = format!("{}.service", prefix);
    let src = artifacts_dir.join(&timer_service);
    let service_path = dst_dir.join(&timer_service);
    if let Err(err) = fs::copy(&src, &service_path) {
        let name = prefix.split("--").last().unwrap();
        error!("failed to copy timer service {}: {}", name, err);
        remove_timer(prefix);
        return false;
    }

    let timer_unit = format!("{}.timer", prefix);
    let src = artifacts_dir.join(&timer_unit);
    let unit_path = dst_dir.join(&timer_unit);
    if let Err(err) = fs::copy(&src, &unit_path) {
        let name = prefix.split("--").last().unwrap();
        error!("failed to copy timer unit {}: {}", name, err);
        remove_timer(prefix);
        return false;
    }

    true
}

pub fn install_timers(deploy_dir: &Path) -> io::Result<()> {
    let artifacts_dir = deploy_dir.join("artifacts");

    let entries = fs::read_dir(&artifacts_dir)?;
    let mut timers = Vec::new();
    for entry in entries {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(_) => continue,
        };
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if file_name.starts_with("dpl--")
            && let Some(prefix) = file_name.strip_suffix(".timer")
        {
            timers.push(prefix.to_owned());
        }
    }

    timers.retain(|prefix| copy_timer(prefix, &artifacts_dir));

    if timers.is_empty() {
        return Ok(());
    }

    reload_systemd();

    for prefix in &timers {
        let unit = format!("{}.timer", prefix);
        match run_systemctl(&["enable", "--now", &unit]) {
            Ok(_) => {
                info!("timer {} installed", unit)
            }
            Err(err) => {
                remove_timer(prefix);
                error!("failed to install timer {}: {}", unit, err);
            }
        }
        //
    }

    Ok(())
}

/// Stop and remove all stale timer-related unit files for the given entity.
fn uninstall_timers(name: &str) {
    let prefix = format!("dpl--{name}--");
    let dir = Path::new(SYSTEMD_DIR);

    let mut timers: Vec<String> = Vec::new();

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
        if file_name.starts_with(&prefix)
            && let Some(file_name) = file_name.strip_suffix(".timer")
        {
            timers.push(file_name.to_owned());
        }
    }

    if timers.is_empty() {
        return;
    }

    for prefix in &timers {
        remove_timer(prefix);
    }

    reload_systemd();
}

pub fn install_app(name: &str, deploy_dir: &Path) -> io::Result<()> {
    let unit = format!("dpl--{name}.service");
    let src = deploy_dir.join("artifacts").join(&unit);
    let dst = Path::new(SYSTEMD_DIR).join(&unit);

    fs::copy(&src, &dst)?;

    reload_systemd();
    if let Err(err) = run_systemctl(&["enable", "--now", &unit]) {
        let _ = fs::remove_file(&dst);
        return Err(err);
    }

    info!("app service installed and started: {}", unit);
    Ok(())
}

pub fn uninstall_app(name: &str) {
    let app_prefix = format!("dpl--{}", name);
    remove_app(&app_prefix);
    reload_systemd();
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
