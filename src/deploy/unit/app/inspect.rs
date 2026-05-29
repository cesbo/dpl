use super::AppUnit;
use crate::{
    deploy::{
        DeployError,
        DeployState,
        DeployStatus,
        Field,
        Health,
        Section,
        UnitReport,
    },
    podman::inspect_container,
};

impl AppUnit<'_> {
    pub fn inspect(&self) -> Result<UnitReport, DeployError> {
        let mut report = UnitReport::new(self.name.as_str(), "app");
        report.push(self.deploy_section()?);
        report.push(self.container_section());
        Ok(report)
    }

    /// On-disk deploy state: version, status, active version, last error.
    fn deploy_section(&self) -> Result<Section, DeployError> {
        let state = DeployState::load(self.ctx, self.name)?;

        let mut section = Section::new("deploy");
        let build = &state.latest_build;
        let (status, health) = match build.status {
            DeployStatus::Ready => ("ready", Health::Ok),
            DeployStatus::Failed => ("failed", Health::Down),
            DeployStatus::Building => ("building", Health::Warn),
            DeployStatus::Idle => ("idle", Health::Unknown),
        };

        section.push(Field::new("version", build.version.to_string()).health(Health::Ok));
        section.push(Field::new("status", status).health(health));
        if let Some(active) = state.active_version {
            section.push(Field::new("active", active.to_string()));
        }
        if build.status == DeployStatus::Failed
            && let Some(phase) = &build.phase
        {
            section.push(Field::new("phase", phase.clone()).health(Health::Down));
        }

        Ok(section)
    }

    /// Live container state from `podman container inspect`.
    fn container_section(&self) -> Section {
        let mut section = Section::new("container");

        match inspect_container(self.name) {
            Some(c) => {
                let health = match c.state.status.as_str() {
                    "running" => Health::Ok,
                    "created" | "paused" => Health::Warn,
                    _ => Health::Down,
                };
                section.push(Field::new("state", c.state.status).health(health));
                section.push(Field::new("started", c.state.started_at));
                section.push(Field::new("restarts", c.restart_count.to_string()));
                if c.state.exit_code != 0 {
                    section.push(
                        Field::new("exit_code", c.state.exit_code.to_string()).health(Health::Down),
                    );
                }
                section.push(Field::new("image", c.image_name));
            }
            None => {
                section.push(Field::new("state", "unavailable").health(Health::Down));
            }
        }

        section
    }
}
