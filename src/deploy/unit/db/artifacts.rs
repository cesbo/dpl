use std::{
    fs,
    path::Path,
    sync::LazyLock,
};

use minijinja::{
    Environment,
    context,
};

use super::model::DbConfig;
use crate::{
    MainContext,
    deploy::artifacts::{
        ArtifactError,
        render,
    },
};

const DB_SERVICE_TEMPLATE: &str = "db-service";

static TEMPLATES: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(true);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);

    env.add_template(
        DB_SERVICE_TEMPLATE,
        include_str!("templates/db-service.jinja"),
    )
    .unwrap();

    env
});

pub fn service_file_name(name: &str) -> String {
    format!("dpl--{name}.service")
}

pub struct ArtifactsContext<'a> {
    pub ctx: &'a MainContext,
    pub name: &'a str,
    pub config: &'a DbConfig,
    pub version: u32,
    /// Plaintext root password — inlined into the systemd `Environment=` line.
    pub password: &'a str,
}

impl<'a> ArtifactsContext<'a> {
    pub fn save(&self, deploy_dir: &Path) -> Result<(), ArtifactError> {
        let artifacts_dir = deploy_dir.join("artifacts");
        fs::create_dir_all(&artifacts_dir).map_err(ArtifactError::CreateDir)?;

        let engine = self.config.engine;
        let path = artifacts_dir.join(service_file_name(self.name));
        let content = render(
            &TEMPLATES,
            DB_SERVICE_TEMPLATE,
            context! {
                cluster => self.name,
                engine => engine.as_str(),
                version => &self.config.version,
                image => engine.image(&self.config.version),
                data_path => engine.data_path(),
                env_var => engine.password_env(),
                password => escape_systemd_env_value(self.password),
            },
        )?;

        fs::write(&path, content).map_err(ArtifactError::Write)?;

        Ok(())
    }
}

/// Escapes a value for use inside a quoted systemd `Environment="KEY=value"`
/// directive: backslashes and double quotes are backslash-escaped. The caller
/// is responsible for the surrounding `"…"`.
fn escape_systemd_env_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::deploy::unit::db::model::{
        DbConfig,
        DbEngine,
    };

    #[test]
    fn render_db_service() {
        let config = DbConfig {
            engine: DbEngine::Postgresql,
            version: "18".into(),
            secret: "pg-pass".into(),
        };

        let name = "pg-main";
        let temp_dir = tempdir().unwrap();
        let deploy_dir = temp_dir.path().join(name);
        fs::create_dir_all(&deploy_dir).unwrap();

        let ctx = MainContext::default();
        let artifacts = ArtifactsContext {
            ctx: &ctx,
            name,
            config: &config,
            version: 1,
            password: "p@ss",
        };

        artifacts.save(&deploy_dir).unwrap();

        let service_path = deploy_dir.join("artifacts").join("dpl--pg-main.service");
        assert!(service_path.exists());

        let body = fs::read_to_string(&service_path).unwrap();
        assert!(body.contains("--name pg-main"));
        assert!(body.contains("Environment=\"POSTGRES_PASSWORD=p@ss\""));
        assert!(body.contains("-e POSTGRES_PASSWORD"));
        assert!(!body.contains("--secret"));
        assert!(body.contains("-v pg-main-data:/var/lib/postgresql"));
        assert!(body.contains("docker.io/library/postgres:18"));
        assert!(!body.contains("postgres:18-alpine"));
        assert!(body.contains("/var/log/podman/pg-main.log"));
        assert!(body.contains("Description=DPL Database for pg-main (postgresql 18)"));
    }

    #[test]
    fn render_db_service_escapes_password() {
        let config = DbConfig {
            engine: DbEngine::Postgresql,
            version: "18".into(),
            secret: "pg-pass".into(),
        };

        let name = "pg-main";
        let temp_dir = tempdir().unwrap();
        let deploy_dir = temp_dir.path().join(name);
        fs::create_dir_all(&deploy_dir).unwrap();

        let ctx = MainContext::default();
        let artifacts = ArtifactsContext {
            ctx: &ctx,
            name,
            config: &config,
            version: 1,
            password: r#"a\b"c"#,
        };

        artifacts.save(&deploy_dir).unwrap();

        let body = fs::read_to_string(deploy_dir.join("artifacts").join("dpl--pg-main.service"))
            .unwrap();
        assert!(body.contains(r#"Environment="POSTGRES_PASSWORD=a\\b\"c""#));
    }
}
