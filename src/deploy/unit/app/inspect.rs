use super::AppUnit;
use crate::{
    deploy::DeployError,
    log::{
        error_mark,
        print_field,
        success_mark,
    },
    podman::inspect_container,
};

impl AppUnit<'_> {
    /// Print the live container/runtime detail for the app, in the same field
    /// style as the general `dpl inspect` info. Called only when the unit has
    /// an active version.
    pub fn inspect(&self) -> Result<(), DeployError> {
        // A static build-and-export unit never runs a container.
        if self.config.runtime.is_none() {
            print_field(
                "Container",
                format!("{} static export (no container)", success_mark()),
            );
            return Ok(());
        }

        let Some(c) = inspect_container(self.name) else {
            print_field("Container", format!("{} unavailable", error_mark()));
            return Ok(());
        };

        let mark = if c.state.status == "running" {
            success_mark()
        } else {
            error_mark()
        };
        print_field("Container", format!("{mark} {}", c.state.status));
        print_field("Started", &c.state.started_at);
        print_field("Restarts", c.restart_count);
        print_field("Image", &c.image_name);
        if c.state.exit_code != 0 {
            print_field("Exit code", console::style(c.state.exit_code).red());
        }

        Ok(())
    }
}
