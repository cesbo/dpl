use std::{
    fs,
    io,
    path::Path,
};

use tracing::{
    debug,
    error,
};

use crate::config::UnitName;

/// Drop-in directory the system cron daemon scans; changes are picked up
/// automatically, so no reload is needed.
const CRON_D_DIR: &str = "/etc/cron.d";

/// Installs and removes a unit's cron file under `/etc/cron.d`. The file is
/// rendered into the deploy dir's `artifacts/` by [`ArtifactsContext`]; this
/// just copies it into place (or removes a stale one).
///
/// [`ArtifactsContext`]: super::artifacts::ArtifactsContext
pub struct CronContext<'a> {
    cron_dir: &'a Path,
    name: &'a UnitName,
}

impl<'a> CronContext<'a> {
    pub fn new(name: &'a UnitName) -> Self {
        Self {
            cron_dir: Path::new(CRON_D_DIR),
            name,
        }
    }

    /// Copies the rendered cron file into `/etc/cron.d`. A unit with no enabled
    /// timers renders no artifact, in which case this is a no-op.
    pub fn install(&self, deploy_dir: &Path) -> io::Result<()> {
        let file_name = self.name.scoped_unit_name();
        let src = deploy_dir
            .join("artifacts")
            .join(format!("{file_name}.cron"));

        // No artifact means no enabled timers: nothing to install.
        if !src.exists() {
            return Ok(());
        }

        let dst = self.cron_dir.join(&file_name);
        fs::copy(&src, &dst)?;
        debug!("installed cron file for unit {}", self.name);
        Ok(())
    }

    /// Removes the unit's cron file. Best-effort: a missing file is fine.
    pub fn uninstall(&self) {
        let path = self.cron_dir.join(self.name.scoped_unit_name());
        match fs::remove_file(&path) {
            Ok(_) => debug!("removed cron file for unit {}", self.name),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => error!("failed to remove cron file for unit {}: {err}", self.name),
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    fn ctx<'a>(cron_dir: &'a Path, name: &'a UnitName) -> CronContext<'a> {
        CronContext { cron_dir, name }
    }

    #[test]
    fn install_copies_artifact_then_uninstall_removes_it() {
        let cron_dir = tempdir().unwrap();
        let deploy_dir = tempdir().unwrap();
        let name = UnitName::new("my-app").unwrap();

        let artifacts = deploy_dir.path().join("artifacts");
        fs::create_dir_all(&artifacts).unwrap();
        fs::write(artifacts.join("dpl--my-app.cron"), "0 * * * * root x\n").unwrap();

        let cron = ctx(cron_dir.path(), &name);
        cron.install(deploy_dir.path()).unwrap();

        let installed = cron_dir.path().join("dpl--my-app");
        assert!(installed.exists());
        assert_eq!(
            fs::read_to_string(&installed).unwrap(),
            "0 * * * * root x\n"
        );

        cron.uninstall();
        assert!(!installed.exists());
    }

    #[test]
    fn install_without_artifact_is_noop() {
        let cron_dir = tempdir().unwrap();
        let deploy_dir = tempdir().unwrap();
        let name = UnitName::new("no-timers").unwrap();
        fs::create_dir_all(deploy_dir.path().join("artifacts")).unwrap();

        ctx(cron_dir.path(), &name)
            .install(deploy_dir.path())
            .unwrap();

        assert!(!cron_dir.path().join("dpl--no-timers").exists());
    }

    #[test]
    fn uninstall_missing_file_is_noop() {
        let cron_dir = tempdir().unwrap();
        let name = UnitName::new("gone").unwrap();
        // Must not error or panic when there is nothing to remove.
        ctx(cron_dir.path(), &name).uninstall();
    }
}
