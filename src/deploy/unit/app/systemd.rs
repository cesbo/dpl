use std::{
    fs,
    io::{
        self,
        Write,
    },
    path::Path,
};

use tracing::{
    debug,
    error,
};

use crate::{
    config::UnitName,
    systemd::{
        disable_service,
        enable_service,
        run_systemctl,
    },
};

pub struct SystemdContext<'a> {
    systemd_dir: &'a Path,
    name: &'a UnitName,
}

impl<'a> SystemdContext<'a> {
    pub fn new(name: &'a UnitName) -> Self {
        Self {
            systemd_dir: Path::new(crate::systemd::SYSTEMD_DIR),
            name,
        }
    }

    pub fn install_app(&self, deploy_dir: &Path) -> io::Result<()> {
        let service_name = format!("{}.service", self.name.scoped_unit_name());
        let src = deploy_dir.join("artifacts").join(&service_name);
        let dst = self.systemd_dir.join(&service_name);

        fs::copy(&src, &dst)?;

        reload_systemd();

        if let Err(err) = enable_service(&service_name) {
            let _ = fs::remove_file(&dst);
            reload_systemd();
            Err(err)
        } else {
            Ok(())
        }
    }

    pub fn uninstall_app(&self) {
        let prefix = self.name.scoped_unit_name();

        let app_service = format!("{prefix}.service");
        let app_service_path = self.systemd_dir.join(&app_service);
        let _ = disable_service(&app_service);

        // Check if the container failed to stop
        if run_systemctl(&["-q", "is-failed", &app_service]).is_ok() {
            let _ = run_systemctl(&["-q", "reset-failed", &app_service]);
        }

        if let Err(err) = fs::remove_file(&app_service_path) {
            if err.kind() != io::ErrorKind::NotFound {
                error!("failed to remove service for unit {}: {err}", self.name);
            }
        } else {
            debug!("removed service for unit {}", self.name);
            reload_systemd();
        }
    }

    /// Rewrites the installed service file
    pub fn set_restart_value(&self, value: &str) -> io::Result<()> {
        let service_name = format!("{}.service", self.name.scoped_unit_name());
        let path = self.systemd_dir.join(&service_name);

        let mut in_service = false;
        let mut result = Vec::new();

        let content = fs::read_to_string(&path)?;
        for line in content.lines() {
            let line = line.trim();

            if line.starts_with('[') && line.ends_with(']') {
                in_service = line == "[Service]";
                result.push(line.to_string());
                continue;
            }

            if in_service && line.starts_with("Restart=") {
                result.push(format!("Restart={}", value));
                continue;
            }

            result.push(line.to_string());
        }
        let content = result.join("\n");

        let mut tmp = tempfile::NamedTempFile::new_in(self.systemd_dir)?;
        tmp.write_all(content.as_bytes())?;
        tmp.flush()?;
        tmp.persist(&path)?;

        // Reload without log access - only used internally during install_app,
        // which already logs its own reload.
        let _ = crate::systemd::reload();

        Ok(())
    }
}

fn reload_systemd() {
    if let Err(err) = crate::systemd::reload() {
        error!("reload systemd: {err}");
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
    };

    use tempfile::tempdir;

    use super::SystemdContext;
    use crate::config::UnitName;

    #[test]
    fn systemd_set_restart_value() {
        let tmp = tempdir().expect("create temp directory");
        let service_name = "dpl--demo.service";
        let service_path = tmp.path().join(service_name);

        let source = "[Unit]\nDescription=Demo\n\n[Service]\nRestart=no\nExecStart=/usr/bin/true\n";
        fs::write(&service_path, source).expect("write source service file");

        let name = UnitName::new("demo").unwrap();
        let ctx = SystemdContext {
            systemd_dir: Path::new(tmp.path()),
            name: &name,
        };

        ctx.set_restart_value("always")
            .expect("rewrite Restart directive");

        let updated = fs::read_to_string(&service_path).expect("read updated service file");
        assert!(updated.contains("Restart=always"));
        assert!(!updated.contains("Restart=no"));
    }
}
