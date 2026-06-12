mod model;

use std::fs;

pub use self::model::{
    HttpPort,
    HttpServerConfig,
};
use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        DeployError,
        RunError,
    },
    log,
    podman::{
        NGINX_WWW_MOUNT,
        health,
        image_exists,
        pull_image,
    },
    state::DeployState,
};

const HTTP_PORT: u16 = 80;
const GLOBAL_CONFIG_FILE: &str = "00-dpl.conf";
const GLOBAL_CONFIG: &str = include_str!("templates/00-dpl.conf");

#[derive(Debug)]
pub struct HttpServerUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: HttpServerConfig,
}

impl<'a> HttpServerUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a UnitName, config: HttpServerConfig) -> Self {
        Self { ctx, name, config }
    }

    pub fn deploy(self, state: &mut DeployState) -> Result<(), DeployError> {
        self.install_inner()?;

        // Hand the container off to serve, then wait until nginx is listening.
        state.set_check();
        crate::serve::notify(self.ctx);

        let phase_name = format!("waiting for http-server '{}'", self.name);
        log::phase(&phase_name);
        if let Err(err) = health::check(self.name, HTTP_PORT) {
            return Err(DeployError::step_startup(phase_name, err));
        }

        state.set_ready();
        Ok(())
    }

    /// Ask the running nginx to reload its config (`nginx -s reload`).
    pub fn reload(name: &UnitName) -> Result<(), DeployError> {
        let container = name.scoped_unit_name();
        crate::podman::run_podman(&["exec", &container, "nginx", "-s", "reload"])
            .map_err(|e| DeployError::step_install(format!("reload http-server '{name}'"), e))?;

        Ok(())
    }

    /// Make this http-server reflect on-disk config:
    ///   - if its container is running, ask nginx to reload (fast path);
    ///   - otherwise run a full deploy under the unit's own DeployState lock,
    ///     so a dependent unit (e.g. a domain) can trigger the chain.
    pub fn reload_or_deploy(self) -> Result<(), DeployError> {
        if crate::podman::is_running(self.name) {
            log::phase(format!("reloading http-server '{}'", self.name));
            return Self::reload(self.name);
        }

        let (_guard, mut state) = DeployState::acquire(self.ctx, self.name).map_err(|e| {
            DeployError::step_prepare(format!("acquire http-server '{}'", self.name), e)
        })?;

        state
            .begin_deploy(HttpServerConfig::KIND)
            .map_err(|e| DeployError::step_prepare("begin deploy", e))?;

        match self.deploy(&mut state) {
            Ok(()) => Ok(()),
            Err(err) => {
                // No deploy console of its own here; the primary unit's state records the stage.
                if let Some((stage, message)) = err.failure() {
                    state.set_failed(stage, message);
                }
                Err(err)
            }
        }
    }

    /// Prepare the on-host directories nginx bind-mounts: write `00-dpl.conf`
    /// into the unit's conf dir and create the (initially empty) www dir.
    fn install_inner(&self) -> Result<(), DeployError> {
        let conf_dir = self.ctx.http_conf_dir(self.name);

        if !image_exists(&self.config.image) {
            log::phase("downloading http-server image");
            pull_image(&self.config.image)
                .map_err(|e| DeployError::step_install("download http-server image", e))?;
        }

        log::phase("writing http-server config");
        fs::create_dir_all(&conf_dir)
            .map_err(|e| DeployError::step_install("create http-server conf dir", e))?;
        fs::write(conf_dir.join(GLOBAL_CONFIG_FILE), GLOBAL_CONFIG)
            .map_err(|e| DeployError::step_install("write global config for http-server", e))?;

        // Create the www bind source so the mount has a directory to attach to.
        fs::create_dir_all(self.ctx.http_www_dir(self.name))
            .map_err(|e| DeployError::step_install("create http-server www dir", e))?;

        Ok(())
    }

    pub fn inspect(&self) {
        crate::podman::inspect::print_container_state(self.name);
    }

    /// Run the nginx container in the foreground.
    pub fn start(&self) -> Result<(), RunError> {
        let container = self.name.scoped_unit_name();
        let mut cmd = crate::podman::PodmanRun::new(&container)
            .map_err(|e| RunError::new(format!("prepare podman to run '{}'", self.name), e))?;

        cmd.publish(self.config.http_port, HTTP_PORT);
        if let HttpPort::Port(https_port) = self.config.https_port {
            cmd.publish(https_port, 443);
        }

        cmd.volume(
            self.ctx.http_www_dir(self.name).to_string_lossy(),
            NGINX_WWW_MOUNT,
            &["ro"],
        );
        cmd.volume(
            self.ctx.http_conf_dir(self.name).to_string_lossy(),
            "/etc/nginx/conf.d",
            &["ro"],
        );

        cmd.run_foreground(&self.config.image, &self.ctx.runtime_log_path(self.name))
            .map_err(|e| RunError::new("run podman foreground", e))
    }
}
