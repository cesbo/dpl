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

use super::model::{
    DomainConfig,
    ProxyConfig,
    RouteConfig,
};
use crate::{
    MainContext,
    config::RouteLocation,
    deploy::artifacts::{
        ArtifactError,
        render_template,
    },
    error::RefError,
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
    pub name: &'a str,
    pub config: &'a DomainConfig,
}

impl<'a> ArtifactsContext<'a> {
    pub fn save(&self, deploy_dir: &Path) -> Result<(), ArtifactError> {
        let artifacts_dir = deploy_dir.join("artifacts");
        fs::create_dir_all(&artifacts_dir).map_err(ArtifactError::CreateDir)?;

        let proxy = self.config.proxy.as_ref().map(RenderProxy::new);

        let mut routes = Vec::new();
        for route in &self.config.routes {
            routes.push(RenderRoute::new(self.ctx, route)?);
        }

        let path = artifacts_dir.join(format!("{}.conf", self.name));
        let content = render_template(
            &TEMPLATES,
            NGINX_CONFIG_TEMPLATE,
            context! {
                hosts => &self.config.hosts,
                proxy => proxy,
                custom_config => &self.config.custom_config,
                routes => routes,
            },
        )?;

        fs::write(&path, content).map_err(ArtifactError::Write)?;

        Ok(())
    }
}

#[derive(Serialize)]
struct RenderProxy<'a> {
    header: &'a str,
    proxies: &'a [String],
}

impl<'a> RenderProxy<'a> {
    fn new(proxy: &'a ProxyConfig) -> RenderProxy<'a> {
        match proxy {
            ProxyConfig::Cloudflare => RenderProxy {
                header: "",
                proxies: &[],
            },
            ProxyConfig::Fastly => RenderProxy {
                header: "",
                proxies: &[],
            },
            ProxyConfig::Custom { header, proxies } => RenderProxy {
                header: header.as_str(),
                proxies: proxies.as_slice(),
            },
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RenderRoute<'a> {
    ReverseProxy {
        location: &'a RouteLocation,
        target: String,
    },
    ServeFiles {
        location: &'a RouteLocation,
        root: String,
        spa: bool,
    },
}

impl<'a> RenderRoute<'a> {
    fn new(ctx: &MainContext, route: &'a RouteConfig) -> Result<RenderRoute<'a>, RefError> {
        match route {
            RouteConfig::ReverseProxy { location, target } => {
                let render_route = RenderRoute::ReverseProxy {
                    location,
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
                    location,
                    root: root.render(ctx)?,
                    spa: *spa,
                };
                Ok(render_route)
            }
        }
    }
}
