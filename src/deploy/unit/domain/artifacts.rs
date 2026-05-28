use std::{
    fs,
    path::Path,
    sync::LazyLock,
};

use minijinja::{
    Environment,
    context,
};
use serde::Serialize;

use super::{
    model::{
        DomainConfig,
        RouteConfig,
    },
    proxy::ResolvedProxy,
};
use crate::{
    MainContext,
    deploy::artifacts::{
        ArtifactError,
        render_template,
    },
    error::RefError,
    podman::NGINX_WWW_MOUNT,
};

const NGINX_CONFIG_TEMPLATE: &str = "nginx-config";
const NGINX_SERVICE_TEMPLATE: &str = "nginx-service";

/// Container image for the singleton `dpl-nginx` service.
const NGINX_IMAGE: &str = "docker.io/library/nginx:stable";

/// Global nginx config dropped into the `dpl-nginx-conf` volume as
/// `00-dpl.conf` (loads first; included in nginx's `http` context).
const GLOBAL_CONFIG: &str = include_str!("templates/00-dpl.conf");

static TEMPLATES: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(true);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);

    env.add_template(
        NGINX_CONFIG_TEMPLATE,
        include_str!("templates/nginx.conf.jinja"),
    )
    .unwrap();

    env.add_template(
        NGINX_SERVICE_TEMPLATE,
        include_str!("templates/nginx-service.jinja"),
    )
    .unwrap();

    env
});

/// Write the global `00-dpl.conf` into `conf_dir` (the root of the
/// `dpl-nginx-conf` volume).
pub fn write_global_config(conf_dir: &Path) -> Result<(), ArtifactError> {
    let path = conf_dir.join("00-dpl.conf");
    fs::write(&path, GLOBAL_CONFIG).map_err(ArtifactError::Write)
}

/// Render the singleton `dpl-nginx.service` and write it into `systemd_dir`.
pub fn create_nginx_service(systemd_dir: &Path) -> Result<(), ArtifactError> {
    let content = render_template(
        &TEMPLATES,
        NGINX_SERVICE_TEMPLATE,
        context! { image => NGINX_IMAGE, www_mount => NGINX_WWW_MOUNT },
    )?;
    let path = systemd_dir.join("dpl-nginx.service");
    fs::write(&path, content).map_err(ArtifactError::Write)
}

pub struct ArtifactsContext<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a str,
    pub config: &'a DomainConfig,
    pub proxy: Option<&'a ResolvedProxy>,
}

impl<'a> ArtifactsContext<'a> {
    /// Render the nginx config and write it as `<unit-name>.conf` into the given
    /// `conf_dir` (the root of the `dpl-nginx-conf` volume, which the nginx
    /// container mounts at `/etc/nginx/conf.d`).
    pub fn save(&self, conf_dir: &Path) -> Result<(), ArtifactError> {
        let mut routes = Vec::new();
        for route in &self.config.routes {
            routes.push(RenderRoute::new(self.ctx, route)?);
        }

        let content = render_template(
            &TEMPLATES,
            NGINX_CONFIG_TEMPLATE,
            context! {
                hosts => &self.config.hosts,
                proxy => self.proxy,
                custom_config => &self.config.custom_config,
                routes => routes,
            },
        )?;

        fs::create_dir_all(conf_dir).map_err(ArtifactError::CreateDir)?;

        let path = conf_dir.join(format!("{}.conf", self.name));
        fs::write(&path, content).map_err(ArtifactError::Write)?;

        Ok(())
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RenderRoute<'a> {
    ReverseProxy {
        location: &'a str,
        target: String,
    },
    Uwsgi {
        location: &'a str,
        target: String,
    },
    ServeFiles {
        location: &'a str,
        root: String,
        spa: bool,
    },
}

impl<'a> RenderRoute<'a> {
    fn new(ctx: &MainContext, route: &'a RouteConfig) -> Result<RenderRoute<'a>, RefError> {
        match &route {
            RouteConfig::ReverseProxy { location, target } => {
                let render_route = RenderRoute::ReverseProxy {
                    location: location.as_str(),
                    target: target.render(ctx)?,
                };
                Ok(render_route)
            }
            RouteConfig::Uwsgi { location, target } => {
                let render_route = RenderRoute::Uwsgi {
                    location: location.as_str(),
                    target: target.render(ctx)?,
                };
                Ok(render_route)
            }
            RouteConfig::ServeFiles {
                location,
                root,
                spa,
            } => {
                let render_route = RenderRoute::ServeFiles {
                    location: location.as_str(),
                    root: root.render(ctx)?,
                    spa: *spa,
                };
                Ok(render_route)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn render_nginx_service() {
        let temp_dir = tempdir().unwrap();
        let systemd_dir = temp_dir.path();

        create_nginx_service(systemd_dir).unwrap();

        let service_path = systemd_dir.join("dpl-nginx.service");
        assert!(service_path.exists());

        let body = fs::read_to_string(&service_path).unwrap();
        assert!(body.contains("--name dpl-nginx"));
        assert!(body.contains("-v dpl-nginx-conf:/etc/nginx/conf.d"));
        assert!(body.contains("-v dpl-nginx-www:/var/www"));
        assert!(body.contains("docker.io/library/nginx:stable"));
        assert!(body.contains("ExecReload=/usr/bin/podman exec dpl-nginx nginx -s reload"));
    }

    #[test]
    fn write_global_config_drops_file() {
        let temp_dir = tempdir().unwrap();
        let conf_dir = temp_dir.path();

        write_global_config(&conf_dir).unwrap();

        let path = conf_dir.join("00-dpl.conf");
        assert!(path.exists());

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("ssl_session_cache"));
    }
}
