use std::{
    fs,
    path::Path,
    sync::LazyLock,
};

use minijinja::{
    Environment,
    context,
};

use super::HttpServerUnit;
use crate::{
    deploy::artifacts::{
        ArtifactError,
        render_template,
    },
    podman::{
        NGINX_WWW_MOUNT,
        NGINX_WWW_VOLUME,
    },
};

const SERVICE_TEMPLATE: &str = "http-server-service";

/// Global nginx config dropped into the unit's conf volume as `00-dpl.conf`
const GLOBAL_CONFIG: &str = include_str!("templates/00-dpl.conf");

static TEMPLATES: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(true);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);

    env.add_template(
        SERVICE_TEMPLATE,
        include_str!("templates/http-server-service.jinja"),
    )
    .unwrap();

    env
});

/// Write the global `00-dpl.conf` into `conf_dir` (the root of the unit's conf
/// volume).
pub fn write_global_config(conf_dir: &Path) -> Result<(), ArtifactError> {
    let path = conf_dir.join("00-dpl.conf");
    fs::write(&path, GLOBAL_CONFIG).map_err(ArtifactError::Write)
}

/// Render the http-server systemd service and write it into `systemd_dir`.
/// Returns the written file name.
pub fn create_service_file(
    systemd_dir: &Path,
    unit: &HttpServerUnit,
) -> Result<String, ArtifactError> {
    let container_name = unit.name.scoped_unit_name();
    let conf_volume = unit.conf_volume();

    let content = render_template(
        &TEMPLATES,
        SERVICE_TEMPLATE,
        context! {
            name => unit.name.as_str(),
            container_name => container_name,
            image => &unit.config.image,
            https => unit.config.https,
            conf_volume => conf_volume,
            www_volume => NGINX_WWW_VOLUME,
            www_mount => NGINX_WWW_MOUNT,
        },
    )?;

    let file_name = format!("{container_name}.service");
    let path = systemd_dir.join(&file_name);
    fs::write(&path, content).map_err(ArtifactError::Write)?;

    Ok(file_name)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{
        super::HttpServerConfig,
        *,
    };
    use crate::{
        MainContext,
        config::ResourceName,
    };

    #[test]
    fn write_global_config_drops_file() {
        let temp_dir = tempdir().unwrap();
        let conf_dir = temp_dir.path();

        write_global_config(conf_dir).unwrap();

        let path = conf_dir.join("00-dpl.conf");
        assert!(path.exists());

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("ssl_session_cache"));
        assert!(body.contains("access_log /dev/stdout json;"));
    }

    #[test]
    fn render_service_https_off() {
        let temp_dir = tempdir().unwrap();
        let systemd_dir = temp_dir.path();
        let name = ResourceName::new("web").unwrap();
        let ctx = MainContext::default();
        let unit = HttpServerUnit::new(
            &ctx,
            &name,
            HttpServerConfig {
                image: "docker.io/library/nginx:stable".into(),
                https: false,
            },
        );

        let file_name = create_service_file(systemd_dir, &unit).unwrap();
        assert_eq!(file_name, "dpl--web.service");

        let body = fs::read_to_string(systemd_dir.join(&file_name)).unwrap();
        assert!(body.contains("--name dpl--web"));
        assert!(body.contains("-p 80:80"));
        assert!(!body.contains("443:443"));
        assert!(body.contains("-v dpl--web-conf:/etc/nginx/conf.d"));
        assert!(body.contains("-v dpl-www:/var/www"));
        assert!(body.contains("docker.io/library/nginx:stable"));
        assert!(body.contains("ExecReload=/usr/bin/podman exec dpl--web nginx -s reload"));
        assert!(body.contains("/var/log/podman/dpl--web.log"));
        assert!(body.contains("--log-opt=max-size=20mb"));
        assert!(body.contains("Description=DPL HTTP server for web"));
    }

    #[test]
    fn render_service_https_on() {
        let temp_dir = tempdir().unwrap();
        let systemd_dir = temp_dir.path();
        let name = ResourceName::new("web").unwrap();
        let ctx = MainContext::default();
        let unit = HttpServerUnit::new(
            &ctx,
            &name,
            HttpServerConfig {
                image: "registry.example/custom-nginx:1.27".into(),
                https: true,
            },
        );

        let file_name = create_service_file(systemd_dir, &unit).unwrap();
        let body = fs::read_to_string(systemd_dir.join(&file_name)).unwrap();
        assert!(body.contains("-p 80:80"));
        assert!(body.contains("-p 443:443"));
        assert!(body.contains("registry.example/custom-nginx:1.27"));
    }
}
