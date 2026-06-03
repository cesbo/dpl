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
    MainContext,
    artifacts::{
        ArtifactError,
        render_template,
    },
    config::UnitName,
    deploy::unit::db::DbServerEngine,
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
    ctx: &MainContext,
    name: &UnitName,
    engine: DbServerEngine,
) -> Result<String, ArtifactError> {
    let file_name = format!("{}.service", name.scoped_unit_name());

    // The service only delegates to `dpl start`/`dpl stop`; the image, data
    // volume, and root secret are resolved at runtime by those commands, so the
    // unit file no longer carries the password in an `Environment=` directive.
    let dpl_bin = std::env::current_exe().map_err(ArtifactError::CurrentExe)?;

    let content = render_template(
        &TEMPLATES,
        DB_SERVICE_TEMPLATE,
        context! {
            dpl_bin => dpl_bin.to_string_lossy(),
            dpl_base => ctx.base().to_string_lossy(),
            name => name,
            engine => engine.as_str(),
        },
    )?;

    let path = dst.join(&file_name);
    fs::write(&path, content).map_err(ArtifactError::Write)?;

    Ok(file_name)
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
        let ctx = MainContext::default();

        create_service_file(dst, &ctx, &name, DbServerEngine::Postgresql).unwrap();

        let service_path = dst.join("dpl--pg-main.service");
        assert!(service_path.exists());

        let body = fs::read_to_string(&service_path).unwrap();
        // The service only delegates; no podman flags or secrets in the file.
        assert!(
            body.contains("start pg-main"),
            "missing start delegation:\n{body}"
        );
        assert!(
            body.contains("stop pg-main"),
            "missing stop delegation:\n{body}"
        );
        assert!(
            !body.contains("Environment="),
            "secret must not be inlined into the unit file:\n{body}"
        );
        assert!(
            !body.contains("POSTGRES_PASSWORD") && !body.contains("podman run"),
            "service must not embed container logic:\n{body}"
        );
        assert!(body.contains("Description=DPL Database for pg-main (postgresql)"));
    }
}
