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
use serde::Serialize;
use tokio::fs;

use super::AppConfig;
use crate::artifacts::{
    ArtifactError,
    render,
};

const CONTAINERFILE_TEMPLATE: &str = "containerfile";
const BUILD_SH_TEMPLATE: &str = "build-sh";
const RUN_SH_TEMPLATE: &str = "run-sh";
const APP_SERVICE_TEMPLATE: &str = "app-service";
const TIMER_SERVICE_TEMPLATE: &str = "timer-service";
const TIMER_UNIT_TEMPLATE: &str = "timer-unit";

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
        APP_SERVICE_TEMPLATE,
        include_str!("templates/app-service.jinja"),
    )
    .unwrap();
    env.add_template(
        TIMER_SERVICE_TEMPLATE,
        include_str!("templates/timer-service.jinja"),
    )
    .unwrap();
    env.add_template(
        TIMER_UNIT_TEMPLATE,
        include_str!("templates/timer-unit.jinja"),
    )
    .unwrap();

    env
});

pub struct ArtifactsContext<'a> {
    pub name: &'a str,
    pub config: &'a AppConfig,
    pub version: u32,
    pub port: u16,
}

impl<'a> ArtifactsContext<'a> {
    pub async fn save(&self, deploy_dir: &Path) -> Result<(), ArtifactError> {
        let artifacts_dir = deploy_dir.join("artifacts");

        if let Err(source) = fs::create_dir_all(&artifacts_dir).await {
            return Err(ArtifactError::CreateDir {
                path: artifacts_dir,
                source,
            });
        }

        let path = artifacts_dir.join("containerfile");
        write_artifact(
            path,
            CONTAINERFILE_TEMPLATE,
            context! {
                image => self.config.image,
                port => self.config.port,
                layers => &self.config.build,
            },
        )
        .await?;

        let path = artifacts_dir.join("run.sh");
        write_artifact(
            path,
            RUN_SH_TEMPLATE,
            context! {
                env => &self.config.runtime.env,
                init => &self.config.runtime.init,
                cmd => &self.config.runtime.cmd,
                timers => &self.config.timers,
            },
        )
        .await?;

        for (index, layer) in self.config.build.iter().enumerate() {
            let Some(script) = &layer.script else {
                continue;
            };

            let path = artifacts_dir.join(format!("build-{}.sh", index + 1));
            write_artifact(
                path,
                BUILD_SH_TEMPLATE,
                context! {
                    env => &layer.env,
                    script => script,
                },
            )
            .await?;
        }

        let unit = format!("dpl--{}.service", &self.name);
        let path = artifacts_dir.join(&unit);
        write_artifact(
            path,
            APP_SERVICE_TEMPLATE,
            context! {
                name => &self.name,
                version => self.version,
                host_port => self.port,
                container_port => &self.config.port,
                volumes => &self.config.volumes,
            },
        )
        .await?;

        for timer in &self.config.timers {
            let unit = format!("dpl--{}--{}.service", &self.name, &timer.name);
            let path = artifacts_dir.join(&unit);
            write_artifact(
                path,
                TIMER_SERVICE_TEMPLATE,
                context! {
                    name => &self.name,
                    timer_name => &timer.name,
                },
            )
            .await?;

            let unit = format!("dpl--{}--{}.timer", &self.name, &timer.name);
            let path = artifacts_dir.join(&unit);
            write_artifact(
                path,
                TIMER_UNIT_TEMPLATE,
                context! {
                    name => &self.name,
                    timer_name => &timer.name,
                    schedule => &timer.schedule,
                },
            )
            .await?;
        }

        Ok(())
    }
}

async fn write_artifact<S>(path: PathBuf, name: &str, ctx: S) -> Result<(), ArtifactError>
where
    S: Serialize,
{
    let content = render(&TEMPLATES, name, ctx)?;

    fs::write(&path, content)
        .await
        .map_err(|source| ArtifactError::Write { path, source })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use tempfile::tempdir;

    use super::*;
    use crate::deploy::app_entity::model::{
        AppConfig,
        BuildLayerConfig,
        RuntimeConfig,
        TimerConfig,
    };

    #[tokio::test]
    async fn render_templates() {
        let config = AppConfig {
            image: "ghcr.io/example/demo:latest".into(),
            port: 8080,
            build: vec![
                // without files
                BuildLayerConfig {
                    files: Vec::new(),
                    env: BTreeMap::new(),
                    script: Some("date".to_owned()),
                },
                // with some files
                BuildLayerConfig {
                    files: vec!["package.json".to_owned(), "package-lock.json".to_owned()],
                    env: BTreeMap::new(),
                    script: Some("npm ci".to_owned()),
                },
                // without script
                BuildLayerConfig {
                    files: vec!["test.txt".to_owned()],
                    env: BTreeMap::new(),
                    script: None,
                },
                // copy all
                BuildLayerConfig {
                    files: vec!["*".to_owned()],
                    env: {
                        let mut map = BTreeMap::new();
                        map.insert("SITE_ID".to_owned(), "hello-world".to_owned());
                        map
                    },
                    script: Some("npm run build".to_owned()),
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
            volumes: Vec::new(),
            exports: Vec::new(),
            timers: vec![
                TimerConfig {
                    name: "cleanup".into(),
                    schedule: "*-*-* 03:00:00".into(),
                    script: "echo cleanup".into(),
                },
                TimerConfig {
                    name: "sync".into(),
                    schedule: "hourly".into(),
                    script: "echo sync".into(),
                },
            ],
        };

        let name = "my-app";
        let temp_dir = tempdir().unwrap();
        let deploy_dir = temp_dir.path().join(name);
        fs::create_dir_all(&deploy_dir).await.unwrap();

        let artifacts = ArtifactsContext {
            name,
            config: &config,
            version: 1,
            port: 32323,
        };

        artifacts.save(&deploy_dir).await.unwrap();

        let artifacts_dir = deploy_dir.join("artifacts");
        assert!(artifacts_dir.join("containerfile").exists());
        assert!(artifacts_dir.join("run.sh").exists());
        assert!(artifacts_dir.join("build-1.sh").exists());
        assert!(artifacts_dir.join("build-2.sh").exists());
        assert!(!artifacts_dir.join("build-3.sh").exists());
        assert!(artifacts_dir.join("build-4.sh").exists());
        assert!(artifacts_dir.join("dpl--my-app.service").exists());

        // timer service and timer unit files
        assert!(artifacts_dir.join("dpl--my-app--cleanup.service").exists());
        assert!(artifacts_dir.join("dpl--my-app--cleanup.timer").exists());
        assert!(artifacts_dir.join("dpl--my-app--sync.service").exists());
        assert!(artifacts_dir.join("dpl--my-app--sync.timer").exists());
    }
}
