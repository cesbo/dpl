use std::sync::LazyLock;

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
    reference::ReferenceError,
};

const NGINX_CONFIG_TEMPLATE: &str = "nginx-config";

/// Escape a string for embedding inside an nginx double-quoted string literal.
fn escape_nginx_quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

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
    pub config: &'a DomainConfig,
    pub proxy: Option<&'a ResolvedProxy>,
}

impl<'a> ArtifactsContext<'a> {
    pub fn render(&self) -> Result<String, ArtifactError> {
        let mut routes = Vec::new();
        for route in &self.config.routes {
            routes.push(RenderRoute::new(self.ctx, route)?);
        }

        render_template(
            &TEMPLATES,
            NGINX_CONFIG_TEMPLATE,
            context! {
                hosts => &self.config.hosts,
                proxy => self.proxy,
                custom_config => &self.config.custom_config,
                routes => routes,
            },
        )
    }
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum RenderRoute<'a> {
    ReverseProxy {
        location: &'a str,
        target: String,
        websocket: bool,
    },
    Uwsgi {
        location: &'a str,
        target: String,
    },
    Fastcgi {
        location: &'a str,
        target: String,
    },
    ServeFiles {
        location: &'a str,
        root: String,
        spa: bool,
    },
    Redirect {
        location: &'a str,
        target: &'a str,
        permanent: bool,
    },
    Return {
        location: &'a str,
        status: u16,
        body: Option<String>,
    },
}

impl<'a> RenderRoute<'a> {
    fn new(ctx: &MainContext, route: &'a RouteConfig) -> Result<RenderRoute<'a>, ReferenceError> {
        match &route {
            RouteConfig::ReverseProxy {
                location,
                target,
                websocket,
            } => {
                let render_route = RenderRoute::ReverseProxy {
                    location: location.as_str(),
                    target: target.render(ctx)?,
                    websocket: *websocket,
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
            RouteConfig::Fastcgi { location, target } => {
                let render_route = RenderRoute::Fastcgi {
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
            RouteConfig::Redirect {
                location,
                target,
                permanent,
            } => {
                let render_route = RenderRoute::Redirect {
                    location: location.as_str(),
                    target: target.as_str(),
                    permanent: *permanent,
                };
                Ok(render_route)
            }
            RouteConfig::Return {
                location,
                status,
                body,
            } => {
                let body = body.as_deref().map(escape_nginx_quoted);
                let render_route = RenderRoute::Return {
                    location: location.as_str(),
                    status: *status,
                    body,
                };
                Ok(render_route)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_nginx_quoted_handles_quotes_and_backslashes() {
        assert_eq!(escape_nginx_quoted("ok"), "ok");
        assert_eq!(escape_nginx_quoted(r#"say "hi""#), r#"say \"hi\""#);
        assert_eq!(escape_nginx_quoted(r"a\b"), r"a\\b");
        // Newlines pass through untouched - nginx quoted strings span lines.
        assert_eq!(escape_nginx_quoted("a\nb"), "a\nb");
    }
}
