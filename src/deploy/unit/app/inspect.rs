use super::AppUnit;
use crate::{
    deploy::DeployError,
    log::{
        error_mark,
        success_mark,
    },
    podman::inspect_container,
};

impl AppUnit<'_> {
    /// Print the live container/runtime detail for the app.
    /// Called only when the unit has an active version.
    pub fn inspect(&self) -> Result<(), DeployError> {
        let Some(c) = inspect_container(self.name) else {
            println!("{} container unavailable", error_mark());
            return Ok(());
        };

        let mark = if c.state.status == "running" {
            success_mark()
        } else {
            error_mark()
        };
        println!("{mark} container {}", c.state.status);
        println!("  {:<9} {}", "started", c.state.started_at);
        println!("  {:<9} {}", "restarts", c.restart_count);
        println!("  {:<9} {}", "image", c.image_name);
        if c.state.exit_code != 0 {
            println!(
                "  {:<9} {}",
                "exit code",
                console::style(c.state.exit_code).red()
            );
        }

        Ok(())
    }
}
