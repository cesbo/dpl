use kdl::KdlNode;
use serde::{
    Deserialize,
    Serialize,
};

use crate::{
    MainContext,
    config::{
        NodeError,
        ValidateConfig,
        set_field,
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub image: String,
    pub port: u16,
    pub build: Vec<BuildLayerConfig>,
    pub runtime: RuntimeConfig,
    #[serde(default)]
    pub volumes: Vec<VolumeConfig>,
    #[serde(default)]
    pub exports: Vec<ExportConfig>,
    #[serde(default)]
    pub timers: Vec<TimerConfig>,
    #[serde(default)]
    pub databases: Vec<String>,
}

/// Configuration for a build layer of the application
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BuildLayerConfig {
    /// Description
    pub description: Option<String>,
    /// Files to include in the build layer
    #[serde(default)]
    pub files: Vec<String>,
    /// Environment variables for the build layer
    #[serde(default)]
    pub env: EnvList,
    /// Shell script to execute for the build layer
    pub script: Option<String>,
}

/// Configuration for the runtime environment of the application
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    /// Environment variables
    #[serde(default)]
    pub env: EnvList,
    /// Shell script to initialize the runtime environment
    pub init: Option<String>,
    /// Command to run the application
    pub cmd: String,
}

/// Creates a bind mount
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VolumeConfig {
    /// Source is a podman volume name or full path to the host directory
    pub source: String,
    /// Path inside the container where the volume will be mounted
    pub path: String,
}

/// Exports static files from the container to the host
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExportConfig {
    /// Path inside the container where static files located
    pub source: String,
    /// URL path where the exported files will be accessible
    pub path: String,
}

/// Timers to start scripts periodically in the container
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TimerConfig {
    /// Name
    pub name: String,
    /// Schedule in systemd OnCalendar format
    pub schedule: String,
    /// Script to run
    pub script: String,
}

impl ValidateConfig for TimerConfig {
    fn validate_config(&self) -> Result<(), String> {
        if !validate::resource_name(&self.name) {
            return Err(format!("invalid timer name: '{}'", self.name));
        }

        if self.schedule.is_empty() {
            return Err(format!("timer '{}' has empty schedule", self.name));
        }

        if self.script.is_empty() {
            return Err(format!("timer '{}' has empty script", self.name));
        }

        Ok(())
    }
}

impl TryFrom<&KdlNode> for VolumeConfig {
    type Error = NodeError;

    fn try_from(node: &KdlNode) -> Result<Self, Self::Error> {
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

impl TryFrom<&KdlNode> for ExportConfig {
    type Error = NodeError;

    fn try_from(node: &KdlNode) -> Result<Self, Self::Error> {
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

impl TryFrom<&KdlNode> for TimerConfig {
    type Error = NodeError;

    fn try_from(node: &KdlNode) -> Result<Self, Self::Error> {
        let name = kdl_args!(node, name: String)?;

        let mut schedule: Option<String> = None;
        let mut script: Option<String> = None;

        if let Some(children) = node.children() {
            for child in children.nodes() {
                let name = child.name().value();
                match name {
                    "schedule" => set_field(&mut schedule, child, name)?,
                    "script" => set_field(&mut script, child, name)?,
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

impl AppConfig {
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
                    unit: db.clone(),
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
        self.runtime.env.validate_config()?;
        for layer in &self.build {
            layer.env.validate_config()?;
        }

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
        VolumeConfig::try_from(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_export(src: &str) -> Result<ExportConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        ExportConfig::try_from(doc.nodes().first().expect("test KDL must have a node"))
    }

    fn parse_timer(src: &str) -> Result<TimerConfig, NodeError> {
        let doc: KdlDocument = src.parse().expect("test KDL must parse");
        TimerConfig::try_from(doc.nodes().first().expect("test KDL must have a node"))
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
        assert_eq!(cfg.name, "run-tasks");
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
        assert_eq!(cfg.name, "run-tasks");
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
}
