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
    artifacts::{
        ArtifactError,
        render_template,
    },
};

const CONTAINERFILE_TEMPLATE: &str = "containerfile";
const BUILD_SH_TEMPLATE: &str = "build-sh";
const RUN_SH_TEMPLATE: &str = "run-sh";

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

    env
});

pub struct ArtifactsContext<'a> {
    pub ctx: &'a MainContext,
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
        config::{
            EnvList,
            UnitName,
        },
        deploy::unit::app::model::*,
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
                    env: EnvList::default(),
                    script: Some("date".to_owned()),
                },
                // with some files
                BuildConfig {
                    description: None,
                    files: vec!["package.json".to_owned(), "package-lock.json".to_owned()],
                    env: EnvList::default(),
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
                    schedule: "0 3 * * *".parse().unwrap(),
                    script: "echo cleanup".into(),
                    disabled: false,
                },
                TimerConfig {
                    name: "sync".into(),
                    description: None,
                    schedule: "0 * * * *".parse().unwrap(),
                    script: "echo sync".into(),
                    disabled: false,
                },
                // disabled timers are configured but never rendered
                TimerConfig {
                    name: "purge".into(),
                    description: None,
                    schedule: "0 0 * * 0".parse().unwrap(),
                    script: "echo purge".into(),
                    disabled: true,
                },
            ],
        };

        let name = UnitName::new("my-app").unwrap();
        let temp_dir = tempdir().unwrap();
        let deploy_dir = temp_dir.path().join(name.as_str());
        fs::create_dir_all(&deploy_dir).unwrap();

        let ctx = MainContext {
            base: temp_dir.path().to_path_buf(),
            master_key: None,
        };

        // `database_deps` loads each referenced unit to classify it; the two db
        // units must exist on disk to land in the `db wait` startup gate.
        for db in ["main-db", "cache-db"] {
            ctx.write_test_unit(
                db,
                "type: db\nserver: pg-main\nuser: app1\nsecret: app1-pass\n",
            );
        }

        let artifacts = ArtifactsContext {
            ctx: &ctx,
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

        // Enabled timers reach the container through run.sh's `timer--<name>`
        // dispatch; the disabled one is skipped. The scheduler (not a cron file)
        // now decides when each fires.
        let run_sh = fs::read_to_string(artifacts_dir.join("run.sh")).unwrap();
        assert!(
            run_sh.contains("timer--cleanup") && run_sh.contains("timer--sync"),
            "missing timer dispatch in run.sh:\n{run_sh}"
        );
        assert!(
            !run_sh.contains("timer--purge"),
            "disabled timer must not be rendered:\n{run_sh}"
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
                env: EnvList::default(),
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
                schedule: "0 * * * *".parse().unwrap(),
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
            config: &config,
        };

        artifacts.save(&deploy_dir).unwrap();

        let artifacts_dir = deploy_dir.join("artifacts");
        // The build still runs: containerfile + build scripts are rendered.
        assert!(artifacts_dir.join("containerfile").exists());
        assert!(artifacts_dir.join("build-1.sh").exists());
        // No runtime → no entrypoint or service.
        assert!(!artifacts_dir.join("run.sh").exists());
        assert!(!artifacts_dir.join("dpl--site.service").exists());

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
