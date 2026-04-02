use std::{
    path::{
        Path,
        PathBuf,
    },
    sync::LazyLock,
};

use minijinja::{
    Environment,
    context,
};
use tokio::fs;

use super::AppConfig;
use crate::error::ArtifactError;

const CONTAINERFILE_TEMPLATE: &str = "containerfile";
const BUILD_SH_TEMPLATE: &str = "build-sh";
const RUN_SH_TEMPLATE: &str = "run-sh";
const SERVICEFILE_TEMPLATE: &str = "servicefile";

static TEMPLATES: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(true);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);

    env.add_function("cuid", || -> String { cuid::cuid1().unwrap() });

    env.add_template(
        CONTAINERFILE_TEMPLATE,
        include_str!("templates/containerfile.jinja"),
    )
    .unwrap();
    env.add_template(BUILD_SH_TEMPLATE, include_str!("templates/build.sh.jinja"))
        .unwrap();
    env.add_template(RUN_SH_TEMPLATE, include_str!("templates/run.sh.jinja"))
        .unwrap();
    env.add_template(
        SERVICEFILE_TEMPLATE,
        include_str!("templates/servicefile.jinja"),
    )
    .unwrap();

    env
});

pub struct EntityContext<'a> {
    pub name: &'a str,
    pub config: &'a AppConfig,
    pub dir: &'a Path,
    pub version: u32,
    pub port: u16,
}

impl<'a> EntityContext<'a> {
    pub async fn write_artifacts(&self) -> Result<(), ArtifactError> {
        let path = self.dir.join("Containerfile");
        let content = TEMPLATES
            .get_template(CONTAINERFILE_TEMPLATE)?
            .render(context! {
                image => self.config.image,
                port => self.config.port,
                layers => &self.config.build,
            })?;
        Self::write_artifact(path, content).await?;

        let path = self.dir.join("run.sh");
        let content = TEMPLATES.get_template(RUN_SH_TEMPLATE)?.render(context! {
            env => &self.config.runtime.env,
            init => &self.config.runtime.init,
            cmd => &self.config.runtime.cmd,
        })?;
        Self::write_artifact(path, content).await?;

        for (index, layer) in self.config.build.iter().enumerate() {
            let path = self.dir.join(format!("build-{}.sh", index + 1));
            let content = TEMPLATES
                .get_template(BUILD_SH_TEMPLATE)?
                .render(context! {
                    env => &layer.env,
                    script => &layer.script,
                })?;
            Self::write_artifact(path, content).await?;
        }

        let image_tag = format!("{}:{}", &self.name, self.version);
        let service_file_name = format!("{}.service", &self.name);
        let path = self.dir.join(service_file_name);
        let content = TEMPLATES
            .get_template(SERVICEFILE_TEMPLATE)?
            .render(context! {
                name => &self.name,
                host_port => self.port,
                container_port => &self.config.port,
                volumes => &self.config.volumes,
                image_tag => &image_tag,
            })?;
        Self::write_artifact(path, content).await?;

        Ok(())
    }

    async fn write_artifact(path: PathBuf, contents: String) -> Result<(), ArtifactError> {
        fs::write(&path, contents)
            .await
            .map_err(|source| ArtifactError::Write { path, source })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::*;
    use crate::deploy::{
        EntityType,
        app_entity::model::{
            AppConfig,
            BuildLayerConfig,
            RuntimeConfig,
        },
    };

    #[tokio::test]
    async fn render_templates() {
        let config = AppConfig {
            entity_type: EntityType::App,
            image: "ghcr.io/example/demo:latest".into(),
            port: 8080,
            build: vec![
                BuildLayerConfig {
                    files: vec!["package.json".to_owned(), "package-lock.json".to_owned()],
                    env: BTreeMap::new(),
                    script: "npm ci".to_owned(),
                },
                BuildLayerConfig {
                    files: vec![".".to_owned()],
                    env: {
                        let mut map = BTreeMap::new();
                        map.insert("SITE_ID".to_owned(), "hello-world".to_owned());
                        map
                    },
                    script: "npm run build".to_owned(),
                },
            ],
            runtime: RuntimeConfig {
                env: {
                    let mut map = BTreeMap::new();
                    map.insert("PORT".to_owned(), "8080".to_owned());
                    map.insert("NODE_ENV".to_owned(), "production".to_owned());
                    map
                },
                init: Some("npm run static-generate\nnpm run migrate".to_owned()),
                cmd: "demo-server".to_owned(),
            },
            domain: None,
            route: None,
            volumes: Vec::new(),
            public: None,
        };

        let name = "demo-app";
        let temp_dir = tempdir().unwrap();
        let entity_dir = temp_dir.path().join(name);
        fs::create_dir_all(&entity_dir).await.unwrap();

        let entity_context = EntityContext {
            name,
            config: &config,
            dir: &entity_dir,
            version: 1,
            port: 32323,
        };

        entity_context.write_artifacts().await.unwrap();

        assert!(entity_dir.join("Containerfile").exists());
        assert!(entity_dir.join("run.sh").exists());
        assert!(entity_dir.join("build-1.sh").exists());
        assert!(entity_dir.join("build-2.sh").exists());
        assert!(entity_dir.join("demo-app.service").exists());
    }
}
