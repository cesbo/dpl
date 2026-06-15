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
        custom_config: &'a str,
    },
    Uwsgi {
        location: &'a str,
        target: String,
        custom_config: &'a str,
    },
    Fastcgi {
        location: &'a str,
        target: String,
        custom_config: &'a str,
    },
    ServeFiles {
        location: &'a str,
        root: String,
        spa: bool,
        custom_config: &'a str,
    },
    Redirect {
        location: &'a str,
        target: &'a str,
        permanent: bool,
        custom_config: &'a str,
    },
    Return {
        location: &'a str,
        status: u16,
        body: Option<String>,
        custom_config: &'a str,
    },
}

impl<'a> RenderRoute<'a> {
    fn new(ctx: &MainContext, route: &'a RouteConfig) -> Result<RenderRoute<'a>, ReferenceError> {
        match &route {
            RouteConfig::ReverseProxy {
                location,
                target,
                websocket,
                custom_config,
            } => {
                let render_route = RenderRoute::ReverseProxy {
                    location: location.as_str(),
                    target: target.render(ctx)?,
                    websocket: *websocket,
                    custom_config,
                };
                Ok(render_route)
            }
            RouteConfig::Uwsgi {
                location,
                target,
                custom_config,
            } => {
                let render_route = RenderRoute::Uwsgi {
                    location: location.as_str(),
                    target: target.render(ctx)?,
                    custom_config,
                };
                Ok(render_route)
            }
            RouteConfig::Fastcgi {
                location,
                target,
                custom_config,
            } => {
                let render_route = RenderRoute::Fastcgi {
                    location: location.as_str(),
                    target: target.render(ctx)?,
                    custom_config,
                };
                Ok(render_route)
            }
            RouteConfig::ServeFiles {
                location,
                root,
                spa,
                custom_config,
            } => {
                let render_route = RenderRoute::ServeFiles {
                    location: location.as_str(),
                    root: root.render(ctx)?,
                    spa: *spa,
                    custom_config,
                };
                Ok(render_route)
            }
            RouteConfig::Redirect {
                location,
                target,
                permanent,
                custom_config,
            } => {
                let render_route = RenderRoute::Redirect {
                    location: location.as_str(),
                    target,
                    permanent: *permanent,
                    custom_config,
                };
                Ok(render_route)
            }
            RouteConfig::Return {
                location,
                status,
                body,
                custom_config,
            } => {
                let body = body.as_deref().map(escape_nginx_quoted);
                let render_route = RenderRoute::Return {
                    location: location.as_str(),
                    status: *status,
                    body,
                    custom_config,
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
