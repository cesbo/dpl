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
    artifacts::{
        ArtifactError,
        render_template,
    },
    config::UnitName,
    reference::ReferenceError,
};

const NGINX_CONFIG_TEMPLATE: &str = "nginx-config";

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

    env
});

pub struct ArtifactsContext<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: &'a DomainConfig,
    pub proxy: Option<&'a ResolvedProxy>,
}

impl<'a> ArtifactsContext<'a> {
    /// Render the nginx config and write it as `<unit-name>.conf` into the
    /// given `conf_dir`.
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
    fn new(ctx: &MainContext, route: &'a RouteConfig) -> Result<RenderRoute<'a>, ReferenceError> {
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
