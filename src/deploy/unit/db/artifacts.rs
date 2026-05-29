use std::{
    fs,
    path::Path,
    sync::LazyLock,
};

use minijinja::{
    Environment,
    context,
};

use crate::{
    config::UnitName,
    deploy::{
        artifacts::{
            ArtifactError,
            render_template,
        },
        unit::db::DbServerEngine,
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

pub fn create_service_file(
    dst: &Path,
    name: &UnitName,
    engine: DbServerEngine,
    version: &str,
    password: &str,
) -> Result<String, ArtifactError> {
    let scoped_service_name = name.scoped_unit_name();
    let file_name = format!("{}.service", scoped_service_name);

    let content = render_template(
        &TEMPLATES,
        DB_SERVICE_TEMPLATE,
        context! {
            name => name,
            container_name => scoped_service_name,
            engine => engine.as_str(),
            version => version,
            image => engine.image(version),
            data_path => engine.data_path(),
            env_var => engine.password_env(),
            password => escape_systemd_env_value(password),
        },
    )?;

    let path = dst.join(&file_name);
    fs::write(&path, content).map_err(ArtifactError::Write)?;

    Ok(file_name)
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
    use crate::deploy::unit::db::model::DbServerEngine;

    #[test]
    fn render_db_service() {
        let name = UnitName::new("pg-main").unwrap();
        let temp_dir = tempdir().unwrap();
        let dst = temp_dir.path();

        create_service_file(dst, &name, DbServerEngine::Postgresql, "18", r#"a\b"c"#).unwrap();

        let service_path = dst.join("dpl--pg-main.service");
        assert!(service_path.exists());

        let body = fs::read_to_string(&service_path).unwrap();
        assert!(body.contains("--name dpl--pg-main"));
        assert!(body.contains(r#"Environment="POSTGRES_PASSWORD=a\\b\"c""#));
        assert!(body.contains("-e POSTGRES_PASSWORD"));
        assert!(!body.contains("--secret"));
        assert!(body.contains("-v dpl--pg-main-data:/var/lib/postgresql"));
        assert!(body.contains("docker.io/library/postgres:18"));
        assert!(!body.contains("postgres:18-alpine"));
        assert!(body.contains("/var/log/podman/dpl--pg-main.log"));
        assert!(body.contains("Description=DPL Database for pg-main (postgresql 18)"));
    }
}
