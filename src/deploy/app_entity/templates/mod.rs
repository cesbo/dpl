use std::sync::LazyLock;

use minijinja::{
    Environment,
    context,
};

use super::{
    AppEntity,
    model::{
        BuildLayerConfig,
        RuntimeConfig,
    },
};

const CONTAINERFILE_TEMPLATE: &str = "containerfile";
const BUILD_SCRIPT_TEMPLATE: &str = "build.sh";
const RUN_SCRIPT_TEMPLATE: &str = "run.sh";
const SERVICE_UNIT_TEMPLATE: &str = "servicefile";

static TEMPLATES: LazyLock<Environment<'static>> = LazyLock::new(|| {
    let mut env = Environment::new();
    env.set_keep_trailing_newline(true);
    env.set_trim_blocks(true);
    env.set_lstrip_blocks(true);

    env.add_function("cuid", || -> String { cuid::cuid1().unwrap() });

    env.add_template(CONTAINERFILE_TEMPLATE, include_str!("containerfile.jinja"))
        .unwrap();
    env.add_template(BUILD_SCRIPT_TEMPLATE, include_str!("build.sh.jinja"))
        .unwrap();
    env.add_template(RUN_SCRIPT_TEMPLATE, include_str!("run.sh.jinja"))
        .unwrap();
    env.add_template(SERVICE_UNIT_TEMPLATE, include_str!("servicefile.jinja"))
        .unwrap();

    env
});

fn render_containerfile(entity: &AppEntity) -> Result<String, minijinja::Error> {
    TEMPLATES
        .get_template(CONTAINERFILE_TEMPLATE)?
        .render(context! {
            image => entity.config.image,
            port => entity.config.port,
            layers => &entity.config.build,
        })
}

fn render_build_script(layer: &BuildLayerConfig) -> Result<String, minijinja::Error> {
    TEMPLATES
        .get_template(BUILD_SCRIPT_TEMPLATE)?
        .render(context! {
            env => &layer.env,
            script => &layer.script,
        })
}

fn render_run_script(runtime: &RuntimeConfig) -> Result<String, minijinja::Error> {
    TEMPLATES
        .get_template(RUN_SCRIPT_TEMPLATE)?
        .render(context! {
            env => &runtime.env,
            init => &runtime.init,
            cmd => &runtime.cmd,
        })
}

fn render_service_unit(entity: &AppEntity) -> Result<String, minijinja::Error> {
    let image_tag = format!("{}:{}", &entity.name, entity.version);
    TEMPLATES
        .get_template(SERVICE_UNIT_TEMPLATE)?
        .render(context! {
            name => &entity.name,
            host_port => &entity.port,
            container_port => &entity.config.port,
            volumes => &entity.config.volumes,
            image_tag => &image_tag,
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::deploy::{
        EntityType,
        app_entity::model::{
            AppConfig,
            BuildLayerConfig,
            RuntimeConfig,
        },
    };

    #[test]
    fn render_containerfile_prints_rendered_output() {
        let entity = AppEntity {
            name: "demo-app".into(),
            dir: "/tmp/demo-app".into(),
            config: AppConfig {
                entity_type: EntityType::App,
                image: "ghcr.io/example/demo:latest".to_owned(),
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
            },
            port: 32323,
            version: 1,
        };

        let rendered = render_containerfile(&entity).expect("containerfile should render");
        println!("{rendered}");

        let rendered =
            render_build_script(&entity.config.build[1]).expect("build.sh should render");
        println!("{rendered}");

        let rendered = render_run_script(&entity.config.runtime).expect("run.sh should render");
        println!("{rendered}");

        let rendered = render_service_unit(&entity).expect("servicefile should render");
        println!("{rendered}");
    }
}
