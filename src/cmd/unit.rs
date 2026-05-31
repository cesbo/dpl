use std::{
    fmt,
    fs,
    io::{
        self,
        Read,
    },
    path::Path,
};

use anyhow::{
    Context,
    Result,
    bail,
};
use chrono::{
    DateTime,
    Utc,
};

use crate::{
    MainContext,
    config::UnitName,
    deploy::{
        BuildFailure,
        DeployError,
        DeployState,
        DeployStatus,
        Stage,
        UnitConfig,
        unit::{
            app::AppUnit,
            db::{
                DbServerUnit,
                DbUnit,
            },
            domain::DomainUnit,
            http_server::HttpServerUnit,
        },
    },
    log::DeployLog,
};

pub fn check(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let _ = load_unit(ctx, name)?;
    println!("ok");
    Ok(())
}

pub fn deploy(ctx: &MainContext, name: &UnitName, path: Option<&Path>) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let input = match unit {
        UnitConfig::App(_) => {
            let input = open_input(path)?;
            if input.is_none() {
                bail!("archive path is required to deploy app unit (use `-` to read from stdin)")
            }
            input
        }
        UnitConfig::Db(_) => open_input(path)?,
        _ => {
            if path.is_some() {
                bail!("path argument is not allowed for {} unit", unit.kind());
            } else {
                None
            }
        }
    };

    let (_guard, mut state) =
        DeployState::acquire(ctx, name).with_context(|| format!("acquire unit '{name}'"))?;

    let version = state
        .bump_version()
        .map_err(|e| DeployError::step_prepare("bump version", e))
        .with_context(|| format!("deploy unit '{name}'"))?;

    let log_path = ctx.build_log_path(name);
    let log = match DeployLog::open(&log_path, name, version) {
        Ok(log) => log,
        Err(err) => {
            let err = DeployError::step_prepare("open deploy log", err);
            state.set_error(&err);
            return Err(anyhow::Error::new(err).context(format!("deploy unit '{name}'")));
        }
    };
    let _default = log.set_default();

    let result: std::result::Result<(), DeployError> = match unit {
        UnitConfig::App(app_config) => {
            // validated Some in the pre-flight match
            AppUnit::new(ctx, name, app_config).deploy(&mut state, version, input.unwrap())
        }
        UnitConfig::Db(db_config) => DbUnit::new(ctx, name, db_config).deploy(&mut state, input),
        UnitConfig::DbServer(db_server_config) => {
            DbServerUnit::new(ctx, name, db_server_config).deploy(&mut state)
        }
        UnitConfig::HttpServer(http_config) => {
            HttpServerUnit::new(ctx, name, http_config).deploy(&mut state)
        }
        UnitConfig::Domain(domain_config) => {
            DomainUnit::new(ctx, name, domain_config).deploy(&mut state)
        }
    };

    match result {
        Ok(()) => {
            log.finish_ok();
            Ok(())
        }
        Err(err) => {
            // Record the failing stage and cause before converting to Reported.
            state.set_error(&err);
            tracing::debug!("deploy failed: {:#}", anyhow::Error::new(err));
            log.finish_err();
            Err(anyhow::Error::new(DeployError::Reported))
        }
    }
}

pub fn inspect(ctx: &MainContext, name: &UnitName) -> Result<()> {
    let unit = load_unit(ctx, name)?;

    let state = DeployState::load(ctx, name).with_context(|| format!("inspect unit '{name}'"))?;
    let build = &state.latest_build;
    let now = Utc::now();

    const ACTIVE_VERSION: &str = "Active version";
    const LATEST_DEPLOY: &str = "Latest deploy";

    print_field("Unit", unit.kind());
    match state.active_version {
        Some(active) => {
            let deployed = if build.status == DeployStatus::Ready && build.version == active {
                format!(" deployed {}", fmt_ago(now, build.updated_at))
            } else {
                String::new()
            };
            let info = format!("{}{}", console::style(active).green(), deployed);
            print_field(ACTIVE_VERSION, info);
        }
        None => print_field(ACTIVE_VERSION, console::style("-").red()),
    }

    match build.status {
        DeployStatus::Idle => {
            print_field(LATEST_DEPLOY, "No deploys yet");
            return Ok(());
        }
        DeployStatus::Building => {
            let info = format!(
                "Version {} in progress · started {}",
                build.version,
                fmt_ago(now, build.updated_at)
            );
            print_field(LATEST_DEPLOY, info);
        }
        DeployStatus::Failed => {
            let info = format!(
                "Version {} build failed · {}",
                build.version,
                fmt_ago(now, build.updated_at)
            );
            print_field(LATEST_DEPLOY, info);

            if let Some(failure) = &build.failure {
                print_failure(ctx, name, failure);
            }
        }
        DeployStatus::Ready => {}
    }

    // Per-unit runtime detail. Run it whenever a version is live.
    if state.active_version.is_none() {
        return Ok(());
    }

    match unit {
        UnitConfig::App(app_config) => {
            AppUnit::new(ctx, name, app_config)
                .inspect()
                .with_context(|| format!("inspect unit '{name}'"))?;
        }
        UnitConfig::Db(_) => {}
        UnitConfig::DbServer(_) => {}
        UnitConfig::Domain(_) => {}
        UnitConfig::HttpServer(_) => {}
    }

    Ok(())
}

/// Print the failure line for a `Failed` build and point at the relevant log.
fn print_failure(ctx: &MainContext, name: &UnitName, failure: &BuildFailure) {
    print_field("Error", &failure.error);

    // A runtime failure (the health check) lives in the container's own log;
    // every earlier stage is in the build log.
    println!();
    if failure.stage == Stage::Runtime {
        let runtime_log_name = format!("{}.log", name.scoped_unit_name());
        let runtime_log_path = Path::new(crate::podman::PODMAN_LOG_DIR).join(runtime_log_name);
        print_field("Runtime log", runtime_log_path.display());
    } else {
        let build_log_path = ctx.build_log_path(name);
        print_field("Build log", build_log_path.display());
    }
}

fn print_field(key: &str, value: impl fmt::Display) {
    let key = format!("{key}:");
    println!("{key:<20} {value}")
}

/// How long ago `then` was relative to `now`: "just now", "2 min ago",
/// "3 hours ago", "5 days ago".
fn fmt_ago(now: DateTime<Utc>, then: DateTime<Utc>) -> String {
    let secs = (now - then).num_seconds();
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{} min ago", secs / 60)
    } else if secs < 86_400 {
        let h = secs / 3600;
        format!("{h} hour{} ago", if h == 1 { "" } else { "s" })
    } else {
        let d = secs / 86_400;
        format!("{d} day{} ago", if d == 1 { "" } else { "s" })
    }
}

/// Open `path` as a deploy input. `-` means stdin.
fn open_input(path: Option<&Path>) -> Result<Option<Box<dyn Read>>, DeployError> {
    let Some(path) = path else {
        return Ok(None);
    };

    let input = if path.as_os_str() == "-" {
        Box::new(io::stdin().lock())
    } else {
        fs::File::open(path)
            .map(|f| Box::new(f) as Box<dyn Read>)
            .map_err(|err| DeployError::step_prepare("open input", err))?
    };

    Ok(Some(input))
}

fn load_unit(ctx: &MainContext, name: &UnitName) -> Result<UnitConfig> {
    let unit = UnitConfig::load(ctx, name)?;

    unit.validate_references(ctx)
        .with_context(|| format!("unit '{name}': broken reference chain"))?;

    Ok(unit)
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    #[test]
    fn fmt_ago_buckets() {
        let now = DateTime::parse_from_rfc3339("2026-05-31T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ago = |secs: i64| fmt_ago(now, now - Duration::seconds(secs));

        assert_eq!(ago(0), "just now");
        assert_eq!(ago(30), "just now");
        assert_eq!(ago(120), "2 min ago");
        assert_eq!(ago(3600), "1 hour ago");
        assert_eq!(ago(7200), "2 hours ago");
        assert_eq!(ago(86_400), "1 day ago");
        assert_eq!(ago(3 * 86_400), "3 days ago");
    }
}
