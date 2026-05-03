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
use tokio::fs;

use super::model::{
    DomainConfig,
    ProxyConfig,
    RouteConfig,
    RouteTarget,
};
use crate::deploy::{
    DeployError,
    artifacts::{
        ArtifactError,
        render,
    },
    state::{
        DeployState,
        DeployStateError,
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
    pub async fn save(&self, deploy_dir: &Path) -> Result<(), DeployError> {
        let artifacts_dir = deploy_dir.join("artifacts");
        if let Err(source) = fs::create_dir_all(&artifacts_dir).await {
            return Err(ArtifactError::CreateDir {
                path: artifacts_dir,
                source,
            }
            .into());
        }

        let proxy = match &self.config.proxy {
            Some(proxy) => Some(render_proxy(proxy).await),
            None => None,
        };

        let mut routes = Vec::new();
        for route in &self.config.routes {
            routes.push(resolve_route(route).await?);
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
            .map_err(|source| ArtifactError::Write { path, source })?;

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

async fn resolve_route(route: &RouteConfig) -> Result<RenderRoute, DeployError> {
    let target = match &route.target {
        RouteTarget::App { unit } => resolve_route_app(unit).await?,
        RouteTarget::Static { unit, spa } => resolve_route_static(unit, *spa).await?,
    };

    Ok(RenderRoute {
        path: route.path.clone(),
        target,
    })
}

async fn resolve_route_app(unit: &str) -> Result<RenderTarget, DeployError> {
    let unit_dir = crate::config().base.join(unit);
    let path = unit_dir.join("port.txt");

    let content = fs::read_to_string(&path)
        .await
        .map_err(|source| DeployError::UnitError {
            info: "read app unit port".into(),
            source,
        })?;

    let port = content
        .trim()
        .parse::<u16>()
        .map_err(|_| DeployError::UnitError {
            info: "invalid app unit port".into(),
            source: io::ErrorKind::InvalidData.into(),
        })?;

    Ok(RenderTarget::App { port })
}

async fn resolve_route_static(unit: &str, spa: bool) -> Result<RenderTarget, DeployError> {
    let unit_dir = crate::config().base.join(unit);
    let state = DeployState::load(&unit_dir)?;
    let version = state
        .active_version
        .ok_or(DeployStateError::NoActiveVersion)?;

    Ok(RenderTarget::Static {
        root: unit_dir
            .join(format!("deploy_{version}"))
            .join("exports")
            .display()
            .to_string(),
        spa,
    })
}
