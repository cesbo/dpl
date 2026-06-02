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
    config::UnitName,
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
    pub name: &'a UnitName,
    pub config: &'a AppConfig,
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
            .builds
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
                port => self.config.runtime.as_ref().map(|r| r.port),
                layers => &layers,
            },
        )?;

        for (index, layer) in self.config.builds.iter().enumerate() {
            let Some(script) = &layer.script else {
                continue;
            };

            let path = artifacts_dir.join(format!("build-{}.sh", index + 1));
            let prefix = format!("builds[{index}].env");
            write_artifact(
                path,
                BUILD_SH_TEMPLATE,
                context! {
                    env => layer.env.resolve(self.ctx, &prefix)?,
                    script => script,
                },
            )?;
        }

        // Service artifacts (run.sh, the systemd `.service`, and timers) only
        // exist for units with a runtime.
        let Some(runtime) = &self.config.runtime else {
            return Ok(());
        };

        let path = artifacts_dir.join("run.sh");
        write_artifact(
            path,
            RUN_SH_TEMPLATE,
            context! {
                env => runtime.env.resolve(self.ctx, "runtime.env")?,
                init => &runtime.init,
                cmd => &runtime.cmd,
                timers => self.config.timers.iter().filter(|t| !t.disabled).collect::<Vec<_>>(),
            },
        )?;

        let dpl_bin = std::env::current_exe().map_err(ArtifactError::CurrentExe)?;

        // The service only delegates to `dpl start`/`dpl stop`; the version,
        // volumes, database wait-gates, and log path are resolved at runtime by
        // those commands, so they no longer belong in the unit file.
        let file_name = format!("{}.service", self.name.scoped_unit_name());
        let path = artifacts_dir.join(&file_name);
        write_artifact(
            path,
            APP_SERVICE_TEMPLATE,
            context! {
                dpl_bin => dpl_bin.to_string_lossy(),
                dpl_base => self.ctx.base().to_string_lossy(),
                name => &self.name,
            },
        )?;

        for timer in &self.config.timers {
            if timer.disabled {
                continue;
            }

            let scoped_timer_name = self.name.scoped_unit_resource(&timer.name);

            let file_name = format!("{}.service", scoped_timer_name);
            let path = artifacts_dir.join(&file_name);
            write_artifact(
                path,
                TIMER_SERVICE_TEMPLATE,
                context! {
                    dpl_bin => dpl_bin.to_string_lossy(),
                    dpl_base => self.ctx.base().to_string_lossy(),
                    name => &self.name,
                    timer_name => &timer.name,
                },
            )?;

            let file_name = format!("{}.timer", scoped_timer_name);
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
    use crate::deploy::{
        EnvList,
        unit::app::model::*,
    };

    #[test]
    fn render_templates() {
        let config = AppConfig {
            image: "ghcr.io/example/demo:latest".into(),
            builds: vec![
                // without files
                BuildConfig {
                    description: None,
                    files: Vec::new(),
                    env: EnvList::new(),
                    script: Some("date".to_owned()),
                },
                // with some files
                BuildConfig {
                    description: None,
                    files: vec!["package.json".to_owned(), "package-lock.json".to_owned()],
                    env: EnvList::new(),
                    script: Some("npm ci".to_owned()),
                },
                // without script - its db references still feed the wait-gate
                // but are never resolved (script-less layers skip rendering).
                BuildConfig {
                    description: None,
                    files: vec!["test.txt".to_owned()],
                    env: serde_yaml::from_str("DB: \"${main-db:url}\"\nCACHE: \"${cache-db:url}\"")
                        .unwrap(),
                    script: None,
                },
                // copy all
                BuildConfig {
                    description: None,
                    files: vec!["*".to_owned()],
                    env: serde_yaml::from_str("SITE_ID: hello-world").unwrap(),
                    script: Some("npm run build".to_owned()),
                },
            ],
            runtime: Some(RuntimeConfig {
                port: 8080,
                env: serde_yaml::from_str("PORT: 8080\nNODE_ENV: production\n").unwrap(),
                init: Some("npm run static-generate\nnpm run migrate".to_owned()),
                cmd: "demo-server".to_owned(),
            }),
            volumes: Vec::new(),
            exports: Vec::new(),
            timers: vec![
                TimerConfig {
                    name: "cleanup".into(),
                    description: None,
                    schedule: "*-*-* 03:00:00".into(),
                    script: "echo cleanup".into(),
                    disabled: false,
                },
                TimerConfig {
                    name: "sync".into(),
                    description: None,
                    schedule: "hourly".into(),
                    script: "echo sync".into(),
                    disabled: false,
                },
                // disabled timers are configured but never rendered
                TimerConfig {
                    name: "purge".into(),
                    description: None,
                    schedule: "weekly".into(),
                    script: "echo purge".into(),
                    disabled: true,
                },
            ],
        };

        let name = UnitName::new("my-app").unwrap();
        let temp_dir = tempdir().unwrap();
        let deploy_dir = temp_dir.path().join(name.as_str());
        fs::create_dir_all(&deploy_dir).unwrap();

        // `database_deps` loads each referenced unit to classify it; the two db
        // units must exist on disk to land in the `db wait` startup gate.
        for db in ["main-db", "cache-db"] {
            let db_dir = temp_dir.path().join(db);
            fs::create_dir_all(&db_dir).unwrap();
            fs::write(
                db_dir.join("config.yaml"),
                "type: db\nserver: pg-main\nuser: app1\nsecret: app1-pass\n",
            )
            .unwrap();
        }

        let ctx = MainContext {
            base: temp_dir.path().to_path_buf(),
            master_key: None,
        };
        let artifacts = ArtifactsContext {
            ctx: &ctx,
            name: &name,
            config: &config,
        };

        artifacts.save(&deploy_dir).unwrap();

        let artifacts_dir = deploy_dir.join("artifacts");

        let containerfile = fs::read_to_string(artifacts_dir.join("containerfile")).unwrap();
        assert!(
            containerfile.contains("EXPOSE 8080") && containerfile.contains("CMD"),
            "runtime image must EXPOSE the port and set CMD:\n{containerfile}"
        );

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
        // disabled timer is skipped entirely
        assert!(!artifacts_dir.join("dpl--my-app--purge.service").exists());
        assert!(!artifacts_dir.join("dpl--my-app--purge.timer").exists());

        // The timer service delegates to `dpl timer` and embeds no podman logic.
        let timer_service =
            fs::read_to_string(artifacts_dir.join("dpl--my-app--cleanup.service")).unwrap();
        assert!(
            timer_service.contains("timer my-app cleanup"),
            "missing `dpl timer` delegation:\n{timer_service}"
        );
        assert!(
            !timer_service.contains("podman exec") && !timer_service.contains("ExecCondition"),
            "timer service must not embed podman logic:\n{timer_service}"
        );

        // The service only delegates to `dpl start`/`dpl stop`; container logic
        // (db wait gates, the podman run, volumes) is resolved at runtime.
        let service = fs::read_to_string(artifacts_dir.join("dpl--my-app.service")).unwrap();
        assert!(
            service.contains("start my-app"),
            "missing `dpl start` delegation:\n{service}"
        );
        assert!(
            service.contains("stop my-app"),
            "missing `dpl stop` delegation:\n{service}"
        );
        assert!(
            !service.contains("podman run") && !service.contains("ExecStartPre"),
            "service must not embed container logic:\n{service}"
        );
        assert!(
            !service.contains("db wait"),
            "db wait gates must move into `dpl start`:\n{service}"
        );
    }

    #[test]
    fn render_templates_static() {
        // A build-and-export unit: no runtime. Timers are configured but must
        // be skipped (they exec into a container the unit never starts).
        let config = AppConfig {
            image: "alpine".into(),
            builds: vec![BuildConfig {
                description: None,
                files: vec!["*".to_owned()],
                env: EnvList::new(),
                script: Some("npm run build".to_owned()),
            }],
            runtime: None,
            volumes: Vec::new(),
            exports: vec![ExportConfig {
                description: None,
                source: "/app/dist".to_owned(),
                path: "/".to_owned(),
            }],
            timers: vec![TimerConfig {
                name: "cleanup".into(),
                description: None,
                schedule: "hourly".into(),
                script: "echo cleanup".into(),
                disabled: false,
            }],
        };

        let name = UnitName::new("site").unwrap();
        let temp_dir = tempdir().unwrap();
        let deploy_dir = temp_dir.path().join(name.as_str());
        fs::create_dir_all(&deploy_dir).unwrap();

        let ctx = MainContext {
            base: temp_dir.path().to_path_buf(),
            master_key: None,
        };
        let artifacts = ArtifactsContext {
            ctx: &ctx,
            name: &name,
            config: &config,
        };

        artifacts.save(&deploy_dir).unwrap();

        let artifacts_dir = deploy_dir.join("artifacts");
        // The build still runs: containerfile + build scripts are rendered.
        assert!(artifacts_dir.join("containerfile").exists());
        assert!(artifacts_dir.join("build-1.sh").exists());
        // No runtime → no entrypoint, service, or timer files.
        assert!(!artifacts_dir.join("run.sh").exists());
        assert!(!artifacts_dir.join("dpl--site.service").exists());
        assert!(!artifacts_dir.join("dpl--site--cleanup.service").exists());
        assert!(!artifacts_dir.join("dpl--site--cleanup.timer").exists());

        // The image has no EXPOSE or CMD - it exists only to be exported from.
        let containerfile = fs::read_to_string(artifacts_dir.join("containerfile")).unwrap();
        assert!(
            !containerfile.contains("EXPOSE"),
            "static image must not EXPOSE a port:\n{containerfile}"
        );
        assert!(
            !containerfile.contains("CMD"),
            "static image must not set a CMD:\n{containerfile}"
        );
        assert!(
            !containerfile.contains("run.sh"),
            "static image must not copy run.sh:\n{containerfile}"
        );
    }
}
