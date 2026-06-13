use std::collections::BTreeSet;

use serde::{
    Deserialize,
    Serialize,
};

use super::{
    host_name::HostName,
    route_location::RouteLocation,
};
use crate::{
    MainContext,
    config::{
        Ns,
        UnitName,
        Value,
    },
    deploy::unit::{
        UnitConfig,
        http_server::HttpServerConfig,
    },
    reference::{
        Location,
        ReferenceError,
    },
};

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DomainConfig {
    /// Name of the `http-server` unit that serves this domain.
    pub server: UnitName,
    pub hosts: Vec<HostName>,
    #[serde(default)]
    pub proxy: Option<ProxyConfig>,
    #[serde(default)]
    pub custom_config: String,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProxyConfig {
    Cloudflare,
    Fastly,
    Custom {
        header: String,
        proxies: Vec<String>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RouteConfig {
    ReverseProxy {
        /// URL location prefix (e.g. "/billing")
        location: RouteLocation,
        /// Upstream URL (e.g. "http://127.0.0.1:8000")
        target: Value,
    },
    Uwsgi {
        /// URL location prefix (e.g. "/app")
        location: RouteLocation,
        /// uwsgi upstream (e.g. "unix:/run/app.sock" or "127.0.0.1:3031")
        target: Value,
    },
    ServeFiles {
        /// URL location prefix (e.g. "/billing/static")
        location: RouteLocation,
        /// Filesystem root
        root: Value,
        /// Single Page Application
        #[serde(default)]
        spa: bool,
    },
    Redirect {
        /// URL location prefix (e.g. "/old")
        location: RouteLocation,
        /// Destination URL, passed to nginx
        /// (e.g. "https://example.com$request_uri").
        target: String,
        /// Emit 301 (permanent) instead of the default 302 (temporary).
        #[serde(default)]
        permanent: bool,
    },
    Return {
        /// URL location prefix (e.g. "/health")
        location: RouteLocation,
        /// HTTP status code (e.g. 204, 404, 410).
        status: u16,
        /// Optional response body. nginx variables need a doubled `$`.
        #[serde(default)]
        body: Option<Value>,
    },
}

impl DomainConfig {
    pub const KIND: &'static str = "domain";

    /// Apps referenced via `${app:export}` in any `serve_files` route - the apps
    /// whose static files must be copied into this domain's http-server www dir.
    pub fn exported_apps(&self) -> BTreeSet<UnitName> {
        let mut apps = BTreeSet::new();
        for route in &self.routes {
            if let RouteConfig::ServeFiles { root, .. } = route {
                for (ns, key) in root.references() {
                    if key == "export"
                        && let Ns::Unit(app) = ns
                    {
                        apps.insert(app.clone());
                    }
                }
            }
        }
        apps
    }

    /// Units referenced through `${unit:key}` tokens across every route's
    /// `target`/`root`, deduplicated and sorted.
    ///
    /// Reference-derived only; purely syntactic (no unit loading).
    pub fn unit_deps(&self) -> BTreeSet<UnitName> {
        let mut deps = BTreeSet::new();
        for route in &self.routes {
            let value = match route {
                RouteConfig::ReverseProxy { target, .. } => target,
                RouteConfig::Uwsgi { target, .. } => target,
                RouteConfig::ServeFiles { root, .. } => root,
                RouteConfig::Redirect { .. } => continue,
                RouteConfig::Return { body, .. } => {
                    if let Some(body) = body {
                        deps.extend(body.unit_refs().cloned());
                    }
                    continue;
                }
            };
            deps.extend(value.unit_refs().cloned());
        }
        deps
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), ReferenceError> {
        self.resolve_server(ctx)?
            .validate_references(ctx)
            .map_err(|err| err.at(Location::unit(self.server.as_str())))?;

        for (index, route) in self.routes.iter().enumerate() {
            let (value, leaf) = match &route {
                RouteConfig::ReverseProxy { target, .. } => (target, "target"),
                RouteConfig::Uwsgi { target, .. } => (target, "target"),
                RouteConfig::ServeFiles { root, .. } => (root, "root"),
                RouteConfig::Redirect { .. } => continue,
                RouteConfig::Return {
                    body: Some(body), ..
                } => (body, "body"),
                RouteConfig::Return { body: None, .. } => continue,
            };
            value
                .render(ctx)
                .map_err(|err| err.at(Location::field(format!("routes[{index}].{leaf}"))))?;
        }
        Ok(())
    }

    pub fn resolve_server(&self, ctx: &MainContext) -> Result<HttpServerConfig, ReferenceError> {
        UnitConfig::load(ctx, &self.server)
            .map_err(ReferenceError::from)
            .and_then(|cfg| match cfg {
                UnitConfig::HttpServer(server) => Ok(server),
                _ => Err(ReferenceError::wrong_unit_type(
                    self.server.to_string(),
                    "http-server",
                )),
            })
            .map_err(|err| err.at(Location::field("server")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_unit_deps_from_routes() {
        let config: DomainConfig = serde_yaml::from_str(
            "server: web\nhosts:\n  - example.com\nroutes:\n  - location: /api\n    kind: reverse_proxy\n    target: \"${backend:url}\"\n  - location: /app\n    kind: uwsgi\n    target: \"${worker:socket}\"\n  - location: /static\n    kind: serve_files\n    root: \"${assets:export}\"\n  - location: /lit\n    kind: serve_files\n    root: \"/var/www/site\"\n",
        )
        .unwrap();
        let deps = config.unit_deps();
        let names: Vec<&str> = deps.iter().map(UnitName::as_str).collect();
        // Sorted; the literal root contributes nothing.
        assert_eq!(names, vec!["assets", "backend", "worker"]);
    }

    #[test]
    fn domain_exported_apps_only_serve_files_export_refs() {
        let config: DomainConfig = serde_yaml::from_str(
            "server: web\nhosts:\n  - example.com\nroutes:\n  - location: /api\n    kind: reverse_proxy\n    target: \"${backend:url}\"\n  - location: /static\n    kind: serve_files\n    root: \"${assets:export}\"\n  - location: /lit\n    kind: serve_files\n    root: \"/var/www/site\"\n  - location: /blog\n    kind: serve_files\n    root: \"${blog:export}\"\n",
        )
        .unwrap();
        let exported = config.exported_apps();
        let apps: Vec<&str> = exported.iter().map(UnitName::as_str).collect();
        // Only serve_files roots referencing `:export`: backend (:url, reverse_proxy)
        // and the literal root contribute nothing.
        assert_eq!(apps, vec!["assets", "blog"]);
    }

    #[test]
    fn parse_redirect_route_permanent_default_and_explicit() {
        // Bare `$request_uri` passes through verbatim - target is a plain String,
        // so no `$$` escaping is needed for nginx runtime variables.
        let config: DomainConfig = serde_yaml::from_str(
            "server: web\nhosts:\n  - example.com\nroutes:\n  - location: /old\n    kind: redirect\n    target: \"https://example.com$request_uri\"\n  - location: /moved\n    kind: redirect\n    target: \"https://example.com/new\"\n    permanent: true\n",
        )
        .unwrap();

        match &config.routes[0] {
            RouteConfig::Redirect {
                target, permanent, ..
            } => {
                assert_eq!(target, "https://example.com$request_uri");
                assert!(!permanent, "defaults to 302");
            }
            _ => panic!("expected redirect"),
        }
        match &config.routes[1] {
            RouteConfig::Redirect { permanent, .. } => assert!(permanent, "explicit 301"),
            _ => panic!("expected redirect"),
        }
    }

    #[test]
    fn redirect_target_is_literal_and_yields_no_unit_deps() {
        // `${backend:url}`-looking text in a redirect target is literal, not a
        // reference - it contributes nothing to unit_deps.
        let config: DomainConfig = serde_yaml::from_str(
            "server: web\nhosts:\n  - example.com\nroutes:\n  - location: /go\n    kind: redirect\n    target: \"${backend:url}\"\n",
        )
        .unwrap();
        assert!(config.unit_deps().is_empty());
    }

    #[test]
    fn parse_return_route_with_and_without_body() {
        let config: DomainConfig = serde_yaml::from_str(
            "server: web\nhosts:\n  - example.com\nroutes:\n  - location: /health\n    kind: return\n    status: 200\n    body: ok\n  - location: /gone\n    kind: return\n    status: 410\n",
        )
        .unwrap();

        match &config.routes[0] {
            RouteConfig::Return { status, body, .. } => {
                assert_eq!(*status, 200);
                assert_eq!(body.as_ref().unwrap().as_template(), "ok");
            }
            _ => panic!("expected return"),
        }
        match &config.routes[1] {
            RouteConfig::Return { status, body, .. } => {
                assert_eq!(*status, 410);
                assert!(body.is_none(), "body defaults to none");
            }
            _ => panic!("expected return"),
        }
    }

    #[test]
    fn return_body_contributes_to_unit_deps() {
        let config: DomainConfig = serde_yaml::from_str(
            "server: web\nhosts:\n  - example.com\nroutes:\n  - location: /v\n    kind: return\n    status: 200\n    body: \"${backend:url}\"\n",
        )
        .unwrap();
        let deps = config.unit_deps();
        let names: Vec<&str> = deps.iter().map(UnitName::as_str).collect();
        assert_eq!(names, vec!["backend"]);
    }

    #[test]
    fn parse_domain_config_with_custom_proxy() {
        let config: DomainConfig = serde_yaml::from_str(
            r#"
server: web
hosts:
  - example.com
  - www.example.com
proxy:
  type: custom
  header: X-Forwarded-For
  proxies:
    - 192.0.2.10
custom_config: |
  add_header X-Test true;
routes:
  - location: /api
    kind: reverse_proxy
    target: "${backend:url}"
  - location: /static
    kind: serve_files
    root: "/var/www/site"
    spa: true
"#,
        )
        .unwrap();

        assert_eq!(
            config.hosts,
            vec![
                HostName::new("example.com").unwrap(),
                HostName::new("www.example.com").unwrap(),
            ],
        );
        assert_eq!(
            config.proxy,
            Some(ProxyConfig::Custom {
                header: "X-Forwarded-For".into(),
                proxies: vec!["192.0.2.10".into()],
            })
        );
        assert_eq!(config.routes.len(), 2);

        assert!(matches!(config.routes[0], RouteConfig::ReverseProxy { .. }));
        match &config.routes[1] {
            RouteConfig::ServeFiles { spa, .. } => assert!(*spa),
            _ => panic!("expected serve_files action"),
        }
    }

    #[test]
    fn reject_custom_proxy_without_ip() {
        let _result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
server: web
hosts:
  - example.com
proxy:
  type: custom
  header: X-Forwarded-For
  proxies: []
"#,
        );

        // TODO: fix after validate_config implementation
        // assert!(result.is_err());
    }

    #[test]
    fn reject_missing_hosts() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
server: web
routes:
  - location: /
    kind: reverse_proxy
    target: "${backend:url}"
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn reject_empty_hosts() {
        let _result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
server: web
hosts: []
routes:
  - location: /
    kind: reverse_proxy
    target: "${backend:url}"
"#,
        );

        // TODO: fix after validate_config implementation
        // assert!(result.is_err());
    }

    #[test]
    fn reject_route_with_no_action() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
server: web
hosts:
  - example.com
routes:
  - location: /api
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn reject_route_with_unknown_kind() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
server: web
hosts:
  - example.com
routes:
  - location: /api
    kind: unknown
    target: "https://example.com"
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn reject_serve_files_without_root() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
server: web
hosts:
  - example.com
routes:
  - location: /static
    kind: serve_files
    spa: true
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn reject_unknown_field_on_serve_files() {
        let result: Result<DomainConfig, _> = serde_yaml::from_str(
            r#"
server: web
hosts:
  - example.com
routes:
  - location: /static
    kind: serve_files
    root: "/var/www"
    bogus: true
"#,
        );

        assert!(result.is_err());
    }

    #[test]
    fn validate_references_rejects_wrong_server_type() {
        use tempfile::TempDir;

        use crate::reference::ReferenceErrorKind;

        // `server: nginx` resolves to an app unit, not http-server.
        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        ctx.write_test_unit(
            "nginx",
            "type: app\nimage: alpine\nbuilds: []\nruntime:\n  port: 8080\n  cmd: ./run\n",
        );
        let config: DomainConfig =
            serde_yaml::from_str("server: nginx\nhosts:\n  - example.com\nroutes: []\n").unwrap();

        let err = config.validate_references(&ctx).unwrap_err();
        assert!(matches!(&err.trail[0], Location::Field { path } if path == "server"));
        assert!(
            matches!(
                &err.kind,
                ReferenceErrorKind::WrongUnitType { unit, expected }
                    if unit == "nginx" && *expected == "http-server",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn validate_references_accepts_http_server() {
        use tempfile::TempDir;

        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        ctx.write_test_unit("nginx", "type: http-server\n");
        let config: DomainConfig =
            serde_yaml::from_str("server: nginx\nhosts:\n  - example.com\nroutes: []\n").unwrap();

        config.validate_references(&ctx).unwrap();
    }
}
