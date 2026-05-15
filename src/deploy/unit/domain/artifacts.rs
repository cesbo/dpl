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
use thiserror::Error;

use super::model::{
    DomainConfig,
    ProxyConfig,
    RouteAction,
    RouteConfig,
};
use crate::{
    MainContext,
    deploy::{
        EnvError,
        artifacts::{
            ArtifactError,
            render,
        },
    },
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
            routes.push(
                RenderRoute::new(self.ctx, route)
                    .map_err(|err| ArtifactError::Resolve(err.into()))?,
            );
        }

        let path = artifacts_dir.join(format!("{}.conf", self.name));
        let content = render(
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
    fn new(proxy: &ProxyConfig) -> RenderProxy<'_> {
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

#[derive(Debug, Error)]
enum ResolveError {
    #[error("route '{path}': {source}")]
    Render {
        path: String,
        #[source]
        source: EnvError,
    },
}
#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RenderAction {
    ReverseProxy { target: String },
    ServeFiles { root: String, spa: bool },
}

#[derive(Serialize)]
struct RenderRoute {
    path: String,
    #[serde(flatten)]
    action: RenderAction,
}

impl RenderRoute {
    fn new(ctx: &MainContext, route: &RouteConfig) -> Result<Self, ResolveError> {
        let render = |value: &crate::deploy::env::Value| {
            value.render(ctx).map_err(|source| ResolveError::Render {
                path: route.path.clone(),
                source,
            })
        };

        let action = match &route.action {
            RouteAction::ReverseProxy { target } => RenderAction::ReverseProxy {
                target: render(target)?,
            },
            RouteAction::ServeFiles { root, spa } => RenderAction::ServeFiles {
                root: render(root)?,
                spa: *spa,
            },
        };

        Ok(RenderRoute {
            path: route.path.clone(),
            action,
        })
    }
}
