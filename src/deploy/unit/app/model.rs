use kdl::{
    KdlDocument,
    KdlEntry,
    KdlNode,
};

use crate::{
    MainContext,
    config::{
        FromKdlNode,
        NodeError,
        ResourceName,
        ValidateConfig,
        integer_node,
        push_field,
        set_field,
        string_node,
    },
    deploy::{
        EnvList,
        UnitConfig,
    },
    error::{
        Location,
        RefError,
    },
    kdl_args,
    validate,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppConfig {
    pub image: String,
    pub port: u16,
    pub build: Vec<BuildConfig>,
    pub runtime: RuntimeConfig,
    pub volumes: Vec<VolumeConfig>,
    pub exports: Vec<ExportConfig>,
    pub timers: Vec<TimerConfig>,
    pub databases: Vec<ResourceName>,
}

/// Configuration for a build layer of the application
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildConfig {
    /// Description
    pub description: Option<String>,
    /// Files to include in the build layer
    pub files: Vec<String>,
    /// Environment variables for the build layer
    pub env: EnvList,
    /// Shell script to execute for the build layer
    pub script: Option<String>,
}

/// Configuration for the runtime environment of the application
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Environment variables
    pub env: EnvList,
    /// Shell script to initialize the runtime environment
    pub init: Option<String>,
    /// Command to run the application
    pub cmd: String,
}

/// Creates a bind mount
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VolumeConfig {
    /// Source is a podman volume name or full path to the host directory
    pub source: String,
    /// Path inside the container where the volume will be mounted
    pub path: String,
}

/// Exports static files from the container to the host
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportConfig {
    /// Path inside the container where static files located
    pub source: String,
    /// URL path where the exported files will be accessible
    pub path: String,
}

/// Timers to start scripts periodically in the container
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TimerConfig {
    /// Name
    pub name: ResourceName,
    /// Schedule in systemd OnCalendar format
    pub schedule: String,
    /// Script to run
    pub script: String,
}

impl ValidateConfig for TimerConfig {
    fn validate_config(&self) -> Result<(), String> {
        if self.schedule.is_empty() {
            return Err(format!("timer '{}' has empty schedule", self.name));
        }

        if self.script.is_empty() {
            return Err(format!("timer '{}' has empty script", self.name));
        }

        Ok(())
    }
}

impl VolumeConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("volume");
        node.entries_mut().push(KdlEntry::new(self.source.clone()));
        node.entries_mut().push(KdlEntry::new(self.path.clone()));
        node
    }
}

impl FromKdlNode for VolumeConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let (source, path) = kdl_args!(node, source: String, path: String)?;

        if let Some(child) = node.children().and_then(|c| c.nodes().first()) {
            let name = child.name().value();
            return Err(NodeError::UnknownField {
                name: name.to_owned(),
                span: child.span(),
            });
        }

        Ok(VolumeConfig { source, path })
    }
}

impl ExportConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("export");
        node.entries_mut().push(KdlEntry::new(self.source.clone()));
        node.entries_mut().push(KdlEntry::new(self.path.clone()));
        node
    }
}

impl FromKdlNode for ExportConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let (source, path) = kdl_args!(node, source: String, path: String)?;

        if let Some(child) = node.children().and_then(|c| c.nodes().first()) {
            let name = child.name().value();
            return Err(NodeError::UnknownField {
                name: name.to_owned(),
                span: child.span(),
            });
        }

        Ok(ExportConfig { source, path })
    }
}

impl BuildConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("build");
        let mut children = KdlDocument::new();
        if let Some(desc) = &self.description {
            children.nodes_mut().push(string_node("description", desc));
        }
        for file in &self.files {
            children.nodes_mut().push(string_node("file", file));
        }
        if !self.env.is_empty() {
            children.nodes_mut().push(self.env.to_kdl_node("env"));
        }
        if let Some(script) = &self.script {
            children.nodes_mut().push(string_node("script", script));
        }
        node.set_children(children);
        node
    }
}

impl FromKdlNode for BuildConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        kdl_args!(node)?;

        let mut description: Option<String> = None;
        let mut files: Vec<String> = Vec::new();
        let mut env: Option<EnvList> = None;
        let mut script: Option<String> = None;

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "description" => set_field(&mut description, child)?,
                    "file" => push_field(&mut files, child)?,
                    "env" => set_field(&mut env, child)?,
                    "script" => set_field(&mut script, child)?,
                    _ => {
                        return Err(NodeError::UnknownField {
                            name: name.to_owned(),
                            span: child.span(),
                        });
                    }
                }
            }
        }

        Ok(BuildConfig {
            description,
            files,
            env: env.unwrap_or_default(),
            script,
        })
    }
}

impl RuntimeConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("runtime");
        let mut children = KdlDocument::new();
        if !self.env.is_empty() {
            children.nodes_mut().push(self.env.to_kdl_node("env"));
        }
        if let Some(init) = &self.init {
            children.nodes_mut().push(string_node("init", init));
        }
        children.nodes_mut().push(string_node("cmd", &self.cmd));
        node.set_children(children);
        node
    }
}

impl FromKdlNode for RuntimeConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        kdl_args!(node)?;

        let mut env: Option<EnvList> = None;
        let mut init: Option<String> = None;
        let mut cmd: Option<String> = None;

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "env" => set_field(&mut env, child)?,
                    "init" => set_field(&mut init, child)?,
                    "cmd" => set_field(&mut cmd, child)?,
                    _ => {
                        return Err(NodeError::UnknownField {
                            name: name.to_owned(),
                            span: child.span(),
                        });
                    }
                }
            }
        }

        Ok(RuntimeConfig {
            env: env.unwrap_or_default(),
            init,
            cmd: cmd.ok_or(NodeError::MissingField {
                name: "cmd",
                span: node.span(),
            })?,
        })
    }
}

impl TimerConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("timer");
        node.entries_mut().push(KdlEntry::from(&self.name));
        let mut children = KdlDocument::new();
        children
            .nodes_mut()
            .push(string_node("schedule", &self.schedule));
        children
            .nodes_mut()
            .push(string_node("script", &self.script));
        node.set_children(children);
        node
    }
}

impl FromKdlNode for TimerConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        let name = kdl_args!(node, name: ResourceName)?;

        let mut schedule: Option<String> = None;
        let mut script: Option<String> = None;

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "schedule" => set_field(&mut schedule, child)?,
                    "script" => set_field(&mut script, child)?,
                    _ => {
                        return Err(NodeError::UnknownField {
                            name: name.to_owned(),
                            span: child.span(),
                        });
                    }
                }
            }
        }

        Ok(TimerConfig {
            name,
            schedule: schedule.ok_or(NodeError::MissingField {
                name: "schedule",
                span: node.span(),
            })?,
            script: script.ok_or(NodeError::MissingField {
                name: "script",
                span: node.span(),
            })?,
        })
    }
}

impl FromKdlNode for AppConfig {
    fn from_kdl_node(node: &KdlNode) -> Result<Self, NodeError> {
        kdl_args!(node)?;

        let mut image: Option<String> = None;
        let mut port: Option<u16> = None;
        let mut runtime: Option<RuntimeConfig> = None;
        let mut build: Vec<BuildConfig> = Vec::new();
        let mut volumes: Vec<VolumeConfig> = Vec::new();
        let mut exports: Vec<ExportConfig> = Vec::new();
        let mut timers: Vec<TimerConfig> = Vec::new();
        let mut databases: Vec<ResourceName> = Vec::new();

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "image" => set_field(&mut image, child)?,
                    "port" => set_field(&mut port, child)?,
                    "runtime" => set_field(&mut runtime, child)?,
                    "build" => push_field(&mut build, child)?,
                    "volume" => push_field(&mut volumes, child)?,
                    "export" => push_field(&mut exports, child)?,
                    "timer" => push_field(&mut timers, child)?,
                    "database" => push_field(&mut databases, child)?,
                    _ => {
                        return Err(NodeError::UnknownField {
                            name: name.to_owned(),
                            span: child.span(),
                        });
                    }
                }
            }
        }

        Ok(AppConfig {
            image: image.ok_or(NodeError::MissingField {
                name: "image",
                span: node.span(),
            })?,
            port: port.ok_or(NodeError::MissingField {
                name: "port",
                span: node.span(),
            })?,
            runtime: runtime.ok_or(NodeError::MissingField {
                name: "runtime",
                span: node.span(),
            })?,
            build,
            volumes,
            exports,
            timers,
            databases,
        })
    }
}

impl AppConfig {
    pub fn to_kdl_node(&self) -> KdlNode {
        let mut node = KdlNode::new("app");
        let mut children = KdlDocument::new();
        children.nodes_mut().push(string_node("image", &self.image));
        children.nodes_mut().push(integer_node("port", self.port));
        for db in &self.databases {
            children
                .nodes_mut()
                .push(string_node("database", db.as_str()));
        }
        for build in &self.build {
            children.nodes_mut().push(build.to_kdl_node());
        }
        children.nodes_mut().push(self.runtime.to_kdl_node());
        for volume in &self.volumes {
            children.nodes_mut().push(volume.to_kdl_node());
        }
        for export in &self.exports {
            children.nodes_mut().push(export.to_kdl_node());
        }
        for timer in &self.timers {
            children.nodes_mut().push(timer.to_kdl_node());
        }
        node.set_children(children);
        node
    }

    pub fn validate_references(&self, ctx: &MainContext) -> Result<(), RefError> {
        self.runtime.env.resolve(ctx, "runtime.env")?;

        for (index, layer) in self.build.iter().enumerate() {
            layer.env.resolve(ctx, &format!("build[{index}].env"))?;
        }

        for (index, db) in self.databases.iter().enumerate() {
            let inner = match UnitConfig::load(ctx, db) {
                Ok(UnitConfig::Db(config)) => {
                    config.validate_references(ctx)?;
                    continue;
                }
                Ok(_) => RefError::WrongUnitType {
                    unit: db.as_str().to_owned(),
                    expected: "db",
                },
                Err(err) => err.into(),
            };
            return Err(inner.at(Location::field(format!("databases[{index}]"))));
        }

        Ok(())
    }

    pub fn resolve_export(
        &self,
        ctx: &MainContext,
        unit_name: &str,
        key: &str,
    ) -> Result<String, RefError> {
        match key {
            "url" => {
                let unit_dir = ctx.base().join(unit_name);
                let port = super::port::read_port(&unit_dir)
                    .map_err(|err| RefError::Export {
                        reason: format!("resolve app port: {err}"),
                    })?
                    .ok_or_else(|| RefError::Export {
                        reason: format!("app '{unit_name}' is not deployed yet"),
                    })?;
                Ok(format!("http://127.0.0.1:{port}"))
            }
            _ => Err(RefError::UnknownExport {
                key: key.to_owned(),
            }),
        }
    }
}

impl ValidateConfig for AppConfig {
    fn validate_config(&self) -> Result<(), String> {
        for timer in &self.timers {
            timer.validate_config()?;
        }

        for export in &self.exports {
            if !validate::url_path(&export.path) {
                return Err(format!("invalid export path: '{}'", export.path));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use kdl::KdlDocument;
    use tempfile::TempDir;

    use super::*;
    use crate::config::FieldError;

    fn parse_volume(src: &str) -> Result<VolumeConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        VolumeConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_export(src: &str) -> Result<ExportConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        ExportConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_timer(src: &str) -> Result<TimerConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        TimerConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_build(src: &str) -> Result<BuildConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        BuildConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_runtime(src: &str) -> Result<RuntimeConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        RuntimeConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_app(src: &str) -> Result<AppConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        AppConfig::from_kdl_node(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn sample_config() -> AppConfig {
        AppConfig {
            image: "alpine".into(),
            port: 8080,
            build: Vec::new(),
            runtime: RuntimeConfig {
                env: EnvList::default(),
                init: None,
                cmd: "./run".into(),
            },
            volumes: Vec::new(),
            exports: Vec::new(),
            timers: Vec::new(),
            databases: Vec::new(),
        }
    }

    #[test]
    fn app_resolve_export_url() {
        let base = TempDir::new().unwrap();
        let unit_dir = base.path().join("web");
        fs::create_dir_all(&unit_dir).unwrap();
        fs::write(unit_dir.join("port.txt"), "12345").unwrap();

        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config = sample_config();
        assert_eq!(
            config.resolve_export(&ctx, "web", "url").unwrap(),
            "http://127.0.0.1:12345"
        );
    }

    #[test]
    fn app_resolve_export_port_missing() {
        let base = TempDir::new().unwrap();
        let ctx = MainContext {
            base: base.path().to_path_buf(),
            master_key: None,
        };
        let config = sample_config();
        let err = config.resolve_export(&ctx, "web", "url").unwrap_err();
        let RefError::Export { reason } = err else {
            panic!("expected Export variant, got {err:?}");
        };
        assert!(
            reason.contains("is not deployed yet"),
            "unexpected reason: {reason}"
        );
    }

    #[test]
    fn kdl_volume_basic() {
        let cfg = parse_volume(r#"volume "/var/lib/app/uploads" "/app/uploads""#).unwrap();
        assert_eq!(cfg.source, "/var/lib/app/uploads");
        assert_eq!(cfg.path, "/app/uploads");
    }

    #[test]
    fn kdl_volume_named_volume() {
        let cfg = parse_volume(r#"volume "uploads-data" "/app/uploads""#).unwrap();
        assert_eq!(cfg.source, "uploads-data");
        assert_eq!(cfg.path, "/app/uploads");
    }

    #[test]
    fn kdl_volume_missing_source() {
        let err = parse_volume("volume").unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "source", .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_volume_missing_path() {
        let err = parse_volume(r#"volume "/var/lib/app/uploads""#).unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "path", .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_volume_unknown_field() {
        let err = parse_volume(
            r#"
            volume "/s" "/p" {
                mode "rw"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "mode"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_volume_extra_args_rejected() {
        let err = parse_volume(r#"volume "/s" "/p" "/extra""#).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_volume_source_not_a_string() {
        let err = parse_volume(r#"volume 5 "/p""#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "source",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_volume_path_not_a_string() {
        let err = parse_volume(r#"volume "/s" 7"#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "path",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_export_basic() {
        let cfg = parse_export(r#"export "/app/staticfiles" "/static""#).unwrap();
        assert_eq!(cfg.source, "/app/staticfiles");
        assert_eq!(cfg.path, "/static");
    }

    #[test]
    fn kdl_export_missing_source() {
        let err = parse_export("export").unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "source", .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_export_missing_path() {
        let err = parse_export(r#"export "/app/staticfiles""#).unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "path", .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_export_unknown_field() {
        let err = parse_export(
            r#"
            export "/s" "/p" {
                kind "static"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "kind"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_export_extra_args_rejected() {
        let err = parse_export(r#"export "/s" "/p" "/extra""#).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_export_path_not_a_string() {
        let err = parse_export(r#"export "/s" 7"#).unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "path",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_basic() {
        let cfg = parse_timer(
            r#"
            timer "run-tasks" {
                schedule "minutely"
                script "python manage.py run_tasks"
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.name.as_str(), "run-tasks");
        assert_eq!(cfg.schedule, "minutely");
        assert_eq!(cfg.script, "python manage.py run_tasks");
    }

    #[test]
    fn kdl_timer_without_description() {
        let cfg = parse_timer(
            r#"
            timer "run-tasks" {
                schedule "minutely"
                script "python manage.py run_tasks"
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.name.as_str(), "run-tasks");
    }

    #[test]
    fn kdl_timer_multiline_script() {
        let cfg = parse_timer(
            r#"
            timer "run-tasks" {
                schedule "minutely"
                script """
                    python manage.py run_tasks
                    """
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.script, "python manage.py run_tasks");
    }

    #[test]
    fn kdl_timer_missing_schedule() {
        let err = parse_timer(
            r#"
            timer "run-tasks" {
                script "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "schedule"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_missing_script() {
        let err = parse_timer(
            r#"
            timer "run-tasks" {
                schedule "minutely"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "script"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_unknown_field() {
        let err = parse_timer(
            r#"
            timer "run-tasks" {
                schedule "minutely"
                script "x"
                user "root"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "user"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_duplicate_field() {
        let err = parse_timer(
            r#"
            timer "run-tasks" {
                schedule "minutely"
                schedule "hourly"
                script "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "schedule"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_without_name_rejected() {
        let err = parse_timer(
            r#"
            timer {
                schedule "minutely"
                script "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(err, NodeError::MissingArg { name: "name", .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_multiple_args_rejected() {
        let err = parse_timer(
            r#"
            timer "run-tasks" "extra" {
                schedule "minutely"
                script "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_named_arg_rejected() {
        let err = parse_timer(
            r#"
            timer name="run-tasks" {
                schedule "minutely"
                script "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::NamedEntry { .. },
                    ..
                } if name == "name",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_name_not_a_string() {
        let err = parse_timer(
            r#"
            timer 5 {
                schedule "minutely"
                script "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "name",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_name_invalid_resource_name() {
        let err = parse_timer(
            r#"
            timer "Bad_Name" {
                schedule "minutely"
                script "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "name",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_timer_field_not_a_string() {
        let err = parse_timer(
            r#"
            timer "run-tasks" {
                schedule 5
                script "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "schedule",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_build_basic() {
        let cfg = parse_build(
            r#"
            build {
                description "install deps"
                file "requirements.txt"
                env {
                    PIP_INDEX_URL "https://pypi.example.com"
                }
                script "pip install -r requirements.txt"
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.description.as_deref(), Some("install deps"));
        assert_eq!(cfg.files, vec!["requirements.txt".to_owned()]);
        assert_eq!(
            cfg.script.as_deref(),
            Some("pip install -r requirements.txt")
        );
        let resolved = cfg.env.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(
            resolved.get("PIP_INDEX_URL").map(String::as_str),
            Some("https://pypi.example.com"),
        );
    }

    #[test]
    fn kdl_build_bare_node() {
        let cfg = parse_build("build").unwrap();
        assert!(cfg.description.is_none());
        assert!(cfg.files.is_empty());
        assert!(cfg.script.is_none());
        assert_eq!(cfg.env, EnvList::default());
    }

    #[test]
    fn kdl_build_empty_block() {
        let cfg = parse_build("build {}").unwrap();
        assert!(cfg.description.is_none());
        assert!(cfg.files.is_empty());
        assert!(cfg.script.is_none());
        assert_eq!(cfg.env, EnvList::default());
    }

    #[test]
    fn kdl_build_multiple_files() {
        let cfg = parse_build(
            r#"
            build {
                file "requirements.txt"
                file "src/*.py"
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.files,
            vec!["requirements.txt".to_owned(), "src/*.py".to_owned()],
        );
    }

    #[test]
    fn kdl_build_multiline_script() {
        let cfg = parse_build(
            r#"
            build {
                script """
                    pip install --no-cache-dir -r requirements.txt
                    """
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.script.as_deref(),
            Some("pip install --no-cache-dir -r requirements.txt"),
        );
    }

    #[test]
    fn kdl_build_positional_arg_rejected() {
        let err = parse_build(r#"build "stray" { script "x" }"#).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_build_unknown_field() {
        let err = parse_build(
            r#"
            build {
                command "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "command"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_build_duplicate_description() {
        let err = parse_build(
            r#"
            build {
                description "a"
                description "b"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "description"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_build_duplicate_script() {
        let err = parse_build(
            r#"
            build {
                script "a"
                script "b"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "script"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_build_duplicate_env() {
        let err = parse_build(
            r#"
            build {
                env { A "1" }
                env { B "2" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "env"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_build_file_not_a_string() {
        let err = parse_build(
            r#"
            build {
                file 5
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "file",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_build_description_not_a_string() {
        let err = parse_build(
            r#"
            build {
                description 5
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "description",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_runtime_basic() {
        let cfg = parse_runtime(
            r#"
            runtime {
                env {
                    ALLOWED_HOSTS "app.example.com"
                }
                init "python manage.py migrate --noinput"
                cmd "gunicorn app.wsgi:application --bind 0.0.0.0:8000"
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.init.as_deref(),
            Some("python manage.py migrate --noinput"),
        );
        assert_eq!(cfg.cmd, "gunicorn app.wsgi:application --bind 0.0.0.0:8000",);
        let resolved = cfg.env.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(
            resolved.get("ALLOWED_HOSTS").map(String::as_str),
            Some("app.example.com"),
        );
    }

    #[test]
    fn kdl_runtime_only_cmd() {
        let cfg = parse_runtime(r#"runtime { cmd "./run" }"#).unwrap();
        assert_eq!(cfg.cmd, "./run");
        assert!(cfg.init.is_none());
        assert_eq!(cfg.env, EnvList::default());
    }

    #[test]
    fn kdl_runtime_multiline_cmd() {
        let cfg = parse_runtime(
            r#"
            runtime {
                cmd """
                    gunicorn app.wsgi:application --bind 0.0.0.0:8000
                    """
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.cmd, "gunicorn app.wsgi:application --bind 0.0.0.0:8000",);
    }

    #[test]
    fn kdl_runtime_multiline_init() {
        let cfg = parse_runtime(
            r#"
            runtime {
                init """
                    python manage.py migrate --noinput
                    """
                cmd "./run"
            }
            "#,
        )
        .unwrap();
        assert_eq!(
            cfg.init.as_deref(),
            Some("python manage.py migrate --noinput"),
        );
    }

    #[test]
    fn kdl_runtime_env_with_template() {
        let cfg = parse_runtime(
            r#"
            runtime {
                env {
                    SECRET_KEY "literal-key"
                }
                cmd "./run"
            }
            "#,
        )
        .unwrap();
        let resolved = cfg.env.resolve(&MainContext::default(), "env").unwrap();
        assert_eq!(
            resolved.get("SECRET_KEY").map(String::as_str),
            Some("literal-key"),
        );
    }

    #[test]
    fn kdl_runtime_missing_cmd() {
        let err = parse_runtime(r#"runtime { init "x" }"#).unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "cmd"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_runtime_bare_node() {
        let err = parse_runtime("runtime").unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "cmd"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_runtime_unknown_field() {
        let err = parse_runtime(
            r#"
            runtime {
                command "x"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "command"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_runtime_duplicate_cmd() {
        let err = parse_runtime(
            r#"
            runtime {
                cmd "a"
                cmd "b"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "cmd"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_runtime_duplicate_env() {
        let err = parse_runtime(
            r#"
            runtime {
                env { A "1" }
                env { B "2" }
                cmd "./run"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "env"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_runtime_duplicate_init() {
        let err = parse_runtime(
            r#"
            runtime {
                init "a"
                init "b"
                cmd "./run"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "init"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_runtime_positional_arg_rejected() {
        let err = parse_runtime(r#"runtime "stray" { cmd "./run" }"#).unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_runtime_cmd_not_a_string() {
        let err = parse_runtime(
            r#"
            runtime {
                cmd 5
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "string", .. },
                    ..
                } if name == "cmd",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_basic() {
        let cfg = parse_app(
            r#"
            app {
                image "python:3.14-slim"
                port 8000

                database "db-test"
                database "cache-db"

                build {
                    description "install deps"
                    file "requirements.txt"
                    script "pip install -r requirements.txt"
                }

                build {
                    script "python manage.py collectstatic --noinput"
                }

                runtime {
                    env { ALLOWED_HOSTS "app.example.com" }
                    init "python manage.py migrate --noinput"
                    cmd "gunicorn app.wsgi:application --bind 0.0.0.0:8000"
                }

                volume "/var/lib/app/uploads" "/app/uploads"

                export "/app/staticfiles" "/static"

                timer "run-tasks" {
                    schedule "minutely"
                    script "python manage.py run_tasks"
                }
            }
            "#,
        )
        .unwrap();

        assert_eq!(cfg.image, "python:3.14-slim");
        assert_eq!(cfg.port, 8000);
        let dbs: Vec<&str> = cfg.databases.iter().map(|n| n.as_str()).collect();
        assert_eq!(dbs, ["db-test", "cache-db"]);
        assert_eq!(cfg.build.len(), 2);
        assert_eq!(cfg.build[0].description.as_deref(), Some("install deps"));
        assert_eq!(cfg.build[0].files, vec!["requirements.txt".to_owned()]);
        assert!(cfg.build[1].description.is_none());
        assert_eq!(
            cfg.runtime.cmd,
            "gunicorn app.wsgi:application --bind 0.0.0.0:8000"
        );
        assert_eq!(
            cfg.runtime.init.as_deref(),
            Some("python manage.py migrate --noinput")
        );
        assert_eq!(cfg.volumes.len(), 1);
        assert_eq!(cfg.volumes[0].source, "/var/lib/app/uploads");
        assert_eq!(cfg.volumes[0].path, "/app/uploads");
        assert_eq!(cfg.exports.len(), 1);
        assert_eq!(cfg.exports[0].source, "/app/staticfiles");
        assert_eq!(cfg.exports[0].path, "/static");
        assert_eq!(cfg.timers.len(), 1);
        assert_eq!(cfg.timers[0].name.as_str(), "run-tasks");
    }

    #[test]
    fn kdl_app_minimal() {
        let cfg = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap();

        assert_eq!(cfg.image, "alpine");
        assert_eq!(cfg.port, 8080);
        assert_eq!(cfg.runtime.cmd, "./run");
        assert!(cfg.build.is_empty());
        assert!(cfg.volumes.is_empty());
        assert!(cfg.exports.is_empty());
        assert!(cfg.timers.is_empty());
        assert!(cfg.databases.is_empty());
    }

    #[test]
    fn kdl_app_missing_image() {
        let err = parse_app(
            r#"
            app {
                port 8000
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "image"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_missing_port() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "port"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_missing_runtime() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::MissingField { name, .. } if *name == "runtime"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_duplicate_image() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                image "debian"
                port 8080
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "image"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_duplicate_port() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
                port 9090
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "port"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_duplicate_runtime() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
                runtime { cmd "./other" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::DuplicateField { name, .. } if name == "runtime"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_unknown_field() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
                replicas 3
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(&err, NodeError::UnknownField { name, .. } if name == "replicas"),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_positional_arg_rejected() {
        let err = parse_app(
            r#"
            app "stray" {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(err, NodeError::UnexpectedArg { .. }),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_port_out_of_range() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                port 70000
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::OutOfRange { min: 0, max: 65535, .. },
                    ..
                } if name == "port",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_port_not_an_integer() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                port "8080"
                runtime { cmd "./run" }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidType { expected: "integer", .. },
                    ..
                } if name == "port",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_database_invalid_resource_name() {
        let err = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
                database "Bad/Name"
            }
            "#,
        )
        .unwrap_err();
        assert!(
            matches!(
                &err,
                NodeError::InvalidField {
                    name,
                    source: FieldError::InvalidValue { .. },
                    ..
                } if name == "database",
            ),
            "unexpected error: {err:?}",
        );
    }

    #[test]
    fn kdl_app_multiple_databases_preserve_order() {
        let cfg = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
                database "first"
                database "second"
                database "third"
            }
            "#,
        )
        .unwrap();
        let dbs: Vec<&str> = cfg.databases.iter().map(|n| n.as_str()).collect();
        assert_eq!(dbs, ["first", "second", "third"]);
    }

    #[test]
    fn kdl_app_multiple_volumes_preserve_order() {
        let cfg = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
                volume "/a/src" "/a/dst"
                volume "/b/src" "/b/dst"
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.volumes.len(), 2);
        assert_eq!(cfg.volumes[0].source, "/a/src");
        assert_eq!(cfg.volumes[1].source, "/b/src");
    }

    #[test]
    fn kdl_app_multiple_builds_preserve_order() {
        let cfg = parse_app(
            r#"
            app {
                image "alpine"
                port 8080
                runtime { cmd "./run" }
                build { description "one" }
                build { description "two" }
            }
            "#,
        )
        .unwrap();
        assert_eq!(cfg.build.len(), 2);
        assert_eq!(cfg.build[0].description.as_deref(), Some("one"));
        assert_eq!(cfg.build[1].description.as_deref(), Some("two"));
    }
}
