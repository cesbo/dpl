use std::{
    io,
    path::Path,
    sync::LazyLock,
};

use minijinja::{
    Environment,
    context,
};
use serde::Serialize;
use thiserror::Error;
use tokio::fs;

use super::model::{
    DomainConfig,
    ProxyConfig,
    RouteConfig,
    RouteTarget,
};
use crate::{
    MainContext,
    deploy::{
        artifacts::{
            ArtifactError,
            render,
        },
        state::{
            DeployState,
            DeployStateError,
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
    pub version: u32,
}

#[derive(Serialize)]
struct RenderProxy<'a> {
    header: &'a str,
    proxies: &'a [String],
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RenderTarget {
    App {
        /// Application port on the host
        port: u16,
    },
    Static {
        /// Path to root directory. Eg. /opt/dpl/unit-name/deplout_xx/exports
        root: String,
        /// Single Page Application (SPA) flag
        spa: bool,
    },
}

#[derive(Serialize)]
struct RenderRoute {
    path: String,
    target: RenderTarget,
}

impl<'a> ArtifactsContext<'a> {
    pub async fn save(&self, deploy_dir: &Path) -> Result<(), ArtifactError> {
        let artifacts_dir = deploy_dir.join("artifacts");
        fs::create_dir_all(&artifacts_dir)
            .await
            .map_err(ArtifactError::CreateDir)?;

        let proxy = match &self.config.proxy {
            Some(proxy) => Some(render_proxy(proxy).await),
            None => None,
        };

        let mut routes = Vec::new();
        for route in &self.config.routes {
            routes.push(
                resolve_route(self.ctx, route)
                    .await
                    .map_err(|err| ArtifactError::Resolve(err.into()))?,
            );
        }

        let path = artifacts_dir.join(format!("{}.conf", self.name));
        let content = render(
            &TEMPLATES,
            NGINX_CONFIG_TEMPLATE,
            context! {
                name => self.name,
                proxy => proxy,
                custom_config => &self.config.custom_config,
                routes => routes,
            },
        )?;

        fs::write(&path, content)
            .await
            .map_err(ArtifactError::Write)?;

        Ok(())
    }
}

async fn render_proxy(proxy: &ProxyConfig) -> RenderProxy<'_> {
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

#[derive(Debug, Error)]
enum ResolveError {
    #[error("unit state")]
    UnitState(DeployStateError),
    #[error("read unit port")]
    ReadUnitPort(#[source] io::Error),
    #[error("parse unit port")]
    ParseUnitPort(#[source] std::num::ParseIntError),
}

async fn resolve_route(
    ctx: &MainContext,
    route: &RouteConfig,
) -> Result<RenderRoute, ResolveError> {
    let target = match &route.target {
        RouteTarget::App { unit } => resolve_route_app(ctx, unit).await?,
        RouteTarget::Static { unit, spa } => resolve_route_static(ctx, unit, *spa).await?,
    };

    Ok(RenderRoute {
        path: route.path.clone(),
        target,
    })
}

async fn resolve_route_app(ctx: &MainContext, unit: &str) -> Result<RenderTarget, ResolveError> {
    let unit_dir = ctx.base().join(unit);
    let _version = DeployState::get_active_version(&unit_dir).map_err(ResolveError::UnitState)?;

    let path = unit_dir.join("port.txt");
    let port = fs::read_to_string(&path)
        .await
        .map_err(ResolveError::ReadUnitPort)?
        .trim()
        .parse::<u16>()
        .map_err(ResolveError::ParseUnitPort)?;

    Ok(RenderTarget::App { port })
}

async fn resolve_route_static(
    ctx: &MainContext,
    unit: &str,
    spa: bool,
) -> Result<RenderTarget, ResolveError> {
    let unit_dir = ctx.base().join(unit);
    let version = DeployState::get_active_version(&unit_dir).map_err(ResolveError::UnitState)?;

    Ok(RenderTarget::Static {
        root: unit_dir
            .join(format!("deploy_{version}"))
            .join("exports")
            .display()
            .to_string(),
        spa,
    })
}
