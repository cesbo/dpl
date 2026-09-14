use std::collections::BTreeMap;

use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    MainContext,
    config::{
        SecretName,
        UnitName,
    },
    deploy::{
        DeployError,
        RunError,
        deployed_version,
        podman_run_with_env,
        wait_ready,
    },
    log,
    podman::{
        image_exists,
        pull_image,
    },
    reference::{
        Location,
        ReferenceError,
    },
    state::DeployState,
};

const DEFAULT_IMAGE: &str = "docker.io/cloudflare/cloudflared:latest";

/// `cloudflared tunnel run` reads a remotely-managed tunnel's token from here.
const TOKEN_ENV: &str = "TUNNEL_TOKEN";

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CloudflareTunnelConfig {
    #[serde(default = "default_image")]
    pub image: String,

    /// Secret holding the tunnel token from the Zero Trust dashboard.
    pub secret: SecretName,
}

fn default_image() -> String {
    DEFAULT_IMAGE.to_string()
}

impl CloudflareTunnelConfig {
    pub const KIND: &'static str = "cloudflare-tunnel";

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), ReferenceError> {
        ctx.resolve_secret(&self.secret)
            .map_err(ReferenceError::from)
            .map_err(|err| err.at(Location::field("secret")))?;
        Ok(())
    }
}

#[derive(Debug)]
pub struct CloudflareTunnelUnit<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a UnitName,
    pub config: CloudflareTunnelConfig,
}

impl<'a> CloudflareTunnelUnit<'a> {
    pub fn new(ctx: &'a MainContext, name: &'a UnitName, config: CloudflareTunnelConfig) -> Self {
        Self { ctx, name, config }
    }

    /// Hand the connector off to `dpl serve` and wait until it stays running.
    pub fn deploy(self, state: &mut DeployState) -> Result<(), DeployError> {
        let token = self.ctx.resolve_secret(&self.config.secret).map_err(|e| {
            DeployError::step_prepare(format!("resolve secret '{}'", &self.config.secret), e)
        })?;

        if !image_exists(&self.config.image) {
            log::phase("downloading cloudflare-tunnel image");
            pull_image(&self.config.image)
                .map_err(|e| DeployError::step_install("download cloudflare-tunnel image", e))?;
        }

        // Snapshot the token into this version's runtime env.
        let env = BTreeMap::from([(TOKEN_ENV.to_string(), token)]);
        crate::podman::env::save(self.ctx, self.name, state.last_version, &env)
            .map_err(|e| DeployError::step_install("save runtime env", e))?;

        // Remove old runtime env
        if let Some(old) = state.active_version {
            let _ = crate::podman::env::remove(self.ctx, self.name, old);
        }

        state.set_check();

        let container = self.name.scoped_unit_name();
        crate::serve::notify_or_warn(self.ctx, &container);

        // The image is distroless (no shell), so the exec-based port probe cannot
        // run: readiness is the worker criterion, "started and stayed running".
        wait_ready(self.name, CloudflareTunnelConfig::KIND, None)?;
        state.set_ready();

        // Routing lives in the dashboard; tell the operator what to enter there.
        log::phase(
            "Zero Trust dashboard -> Public Hostname -> Service: http://dpl--<http-server>:80",
        );

        Ok(())
    }

    pub fn inspect(&self) {
        crate::podman::inspect::print_container_state(self.name);
    }

    /// Run the connector container in the foreground.
    pub fn start(&self) -> Result<(), RunError> {
        let version = deployed_version(self.ctx, self.name, CloudflareTunnelConfig::KIND)?;
        let container = self.name.scoped_unit_name();

        // The token rides in the podman process env (`--env=TUNNEL_TOKEN`), never on argv.
        let cmd = podman_run_with_env(self.ctx, self.name, version)?;

        // ENTRYPOINT is `cloudflared --no-autoupdate`; the args follow the image.
        let mut cmd = cmd.into_command();
        cmd.args([self.config.image.as_str(), "tunnel", "run"]);

        crate::podman::run_foreground(cmd, &container, &self.ctx.runtime_log_path(self.name))
            .map_err(|e| RunError::new("run podman foreground", e))
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::{
        deploy::UnitConfig,
        reference::ReferenceErrorKind,
        secret::{
            MasterKey,
            SecretError,
        },
    };

    #[test]
    fn parse_defaults() {
        let config: CloudflareTunnelConfig =
            serde_yaml::from_str("secret: cf-tunnel-token\n").unwrap();
        assert_eq!(config.image, DEFAULT_IMAGE);
        assert_eq!(config.secret.as_str(), "cf-tunnel-token");
    }

    #[test]
    fn reject_unknown_field() {
        let err =
            serde_yaml::from_str::<CloudflareTunnelConfig>("secret: cf-tunnel-token\nport: 80\n")
                .unwrap_err();
        assert!(
            err.to_string().contains("unknown field"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn secret_is_required() {
        let err = serde_yaml::from_str::<CloudflareTunnelConfig>("image: x\n").unwrap_err();
        assert!(
            err.to_string().contains("missing field `secret`"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn validate_references_fails_on_missing_secret() {
        let base = TempDir::new().unwrap();
        let key = MasterKey::generate(base.path());
        key.save().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: Some(MasterKey::load(base.path()).unwrap()),
        };
        ctx.write_test_unit("cf", "type: cloudflare-tunnel\nsecret: cf-tunnel-token\n");

        let unit = UnitConfig::load(&ctx, &UnitName::new("cf").unwrap()).unwrap();
        let err = unit.validate_references(&ctx).unwrap_err();

        assert!(matches!(&err.trail[0], Location::Field { path } if path == "secret"));
        assert!(
            matches!(err.kind, ReferenceErrorKind::Secret(SecretError::NotFound { ref name }) if name == "cf-tunnel-token"),
            "unexpected kind: {:?}",
            err.kind,
        );
    }
}
