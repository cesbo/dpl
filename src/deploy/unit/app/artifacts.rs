use std::{
    fs,
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

use super::AppConfig;
use crate::{
    MainContext,
    deploy::artifacts::{
        ArtifactError,
        render_template,
    },
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
    pub ctx: &'a MainContext,
    pub name: &'a str,
    pub config: &'a AppConfig,
    pub version: u32,
    pub port: u16,
}

impl<'a> ArtifactsContext<'a> {
    pub fn save(&self, deploy_dir: &Path) -> Result<(), ArtifactError> {
        let artifacts_dir = deploy_dir.join("artifacts");

        fs::create_dir_all(&artifacts_dir).map_err(ArtifactError::CreateDir)?;

        #[derive(Serialize)]
        struct BuildContext<'a> {
            files: &'a Vec<String>,
            script: Option<&'a str>,
        }

        let layers = self
            .config
            .build
            .iter()
            .map(|b| BuildContext {
                files: &b.files,
                script: b.script.as_deref(),
            })
            .collect::<Vec<BuildContext>>();

        let path = artifacts_dir.join("containerfile");
        write_artifact(
            path,
            CONTAINERFILE_TEMPLATE,
            context! {
                image => self.config.image,
                port => self.config.port,
                layers => &layers,
            },
        )?;

        let path = artifacts_dir.join("run.sh");
        write_artifact(
            path,
            RUN_SH_TEMPLATE,
            context! {
                env => self.config.runtime.env.resolve(self.ctx, "runtime.env")?,
                init => &self.config.runtime.init,
                cmd => &self.config.runtime.cmd,
                timers => &self.config.timers,
            },
        )?;

        for (index, layer) in self.config.build.iter().enumerate() {
            let Some(script) = &layer.script else {
                continue;
            };

            let path = artifacts_dir.join(format!("build-{}.sh", index + 1));
            let prefix = format!("build[{index}].env");
            write_artifact(
                path,
                BUILD_SH_TEMPLATE,
                context! {
                    env => layer.env.resolve(self.ctx, &prefix)?,
                    script => script,
                },
            )?;
        }

        let dpl_bin = std::env::current_exe().map_err(ArtifactError::CurrentExe)?;

        let file_name = format!("dpl--{}.service", &self.name);
        let path = artifacts_dir.join(&file_name);
        write_artifact(
            path,
            APP_SERVICE_TEMPLATE,
            context! {
                dpl_bin => dpl_bin.to_string_lossy(),
                dpl_base => self.ctx.base().to_string_lossy(),
                name => &self.name,
                version => self.version,
                host_port => self.port,
                container_port => &self.config.port,
                volumes => &self.config.volumes,
                databases => &self.config.databases,
            },
        )?;

        for timer in &self.config.timers {
            let file_name = format!("dpl--{}--{}.service", &self.name, &timer.name);
            let path = artifacts_dir.join(&file_name);
            write_artifact(
                path,
                TIMER_SERVICE_TEMPLATE,
                context! {
                    name => &self.name,
                    timer_name => &timer.name,
                },
            )?;

            let file_name = format!("dpl--{}--{}.timer", &self.name, &timer.name);
            let path = artifacts_dir.join(&file_name);
            write_artifact(
                path,
                TIMER_UNIT_TEMPLATE,
                context! {
                    name => &self.name,
                    timer_name => &timer.name,
                    schedule => &timer.schedule,
                },
            )?;
        }

        Ok(())
    }
}

fn write_artifact<S>(path: PathBuf, name: &str, ctx: S) -> Result<(), ArtifactError>
where
    S: Serialize,
{
    let content = render_template(&TEMPLATES, name, ctx)?;

    fs::write(&path, content).map_err(ArtifactError::Write)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;
    use crate::{
        config::ResourceName,
        deploy::{
            EnvList,
            unit::app::model::*,
        },
    };

    #[test]
    fn render_templates() {
        let config = AppConfig {
            image: "ghcr.io/example/demo:latest".into(),
            port: 8080,
            build: vec![
                // without files
                BuildLayerConfig {
                    description: None,
                    files: Vec::new(),
                    env: EnvList::new(),
                    script: Some("date".to_owned()),
                },
                // with some files
                BuildLayerConfig {
                    description: None,
                    files: vec!["package.json".to_owned(), "package-lock.json".to_owned()],
                    env: EnvList::new(),
                    script: Some("npm ci".to_owned()),
                },
                // without script
                BuildLayerConfig {
                    description: None,
                    files: vec!["test.txt".to_owned()],
                    env: EnvList::new(),
                    script: None,
                },
                // copy all
                BuildLayerConfig {
                    description: None,
                    files: vec!["*".to_owned()],
                    env: serde_yaml::from_str("SITE_ID: hello-world").unwrap(),
                    script: Some("npm run build".to_owned()),
                },
            ],
            runtime: RuntimeConfig {
                env: serde_yaml::from_str("PORT: 8080\nNODE_ENV: production\n").unwrap(),
                init: Some("npm run static-generate\nnpm run migrate".to_owned()),
                cmd: "demo-server".to_owned(),
            },
            volumes: Vec::new(),
            exports: Vec::new(),
            timers: vec![
                TimerConfig {
                    name: "cleanup".into(),
                    description: None,
                    schedule: "*-*-* 03:00:00".into(),
                    script: "echo cleanup".into(),
                },
                TimerConfig {
                    name: "sync".into(),
                    description: None,
                    schedule: "hourly".into(),
                    script: "echo sync".into(),
                },
            ],
            databases: vec![
                ResourceName::new("main-db").unwrap(),
                ResourceName::new("cache-db").unwrap(),
            ],
        };

        let name = "my-app";
        let temp_dir = tempdir().unwrap();
        let deploy_dir = temp_dir.path().join(name);
        fs::create_dir_all(&deploy_dir).unwrap();

        let ctx = MainContext::default();
        let artifacts = ArtifactsContext {
            ctx: &ctx,
            name,
            config: &config,
            version: 1,
            port: 32323,
        };

        artifacts.save(&deploy_dir).unwrap();

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

        let service = fs::read_to_string(artifacts_dir.join("dpl--my-app.service")).unwrap();
        assert!(
            service.contains("db wait main-db"),
            "missing db wait for main-db:\n{service}"
        );
        assert!(
            service.contains("db wait cache-db"),
            "missing db wait for cache-db:\n{service}"
        );
    }
}
