//! A failed production check in words: a custom app's `production_smoke`
//! promote check found errors production did not show before a build went
//! live (`oxy-api-custom-apps` `custom_apps_build_checks::production`).
//!
//! The subject is the failed check's row (`app_build_checks.id`). Everything
//! the notice says is read from that row and the two it names (the app, the
//! build), when the notice is read or delivered:
//!
//! | Said | Read from |
//! | --- | --- |
//! | the app | `apps.name`, `apps.slug` |
//! | the build | `app_builds.build_id` |
//! | the build before it | `detail.move.previous.build_id` |
//! | the new errors | `detail.new_errors[]` (`{path, viewport, error}`) and `detail.new_errors_omitted` |
//! | whether they can be called new | `detail.move.baseline`: `false` when the attempt had no baseline from before the move, and the notice then says "errors on production", what the check found, never "new errors"; absent (a row from before it was written) reads as `true` |
//! | whether production was rolled back | `detail.rollback` (`{rolled_back, to: {build_id}, reason}`); absent where the app does not roll back automatically |
//!
//! **Capped.** An error's text is a page's own (`TypeError: …`): at most
//! [`MAX_ERRORS`] are quoted, each cut to [`MAX_ERROR_CHARS`], and the rest
//! are counted.

use std::collections::HashMap;

use entity::{app_build_checks, app_builds, apps};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde_json::Value;
use uuid::Uuid;

use super::render::Rendered;
use super::subjects::Record;

/// The most errors one notice quotes.
pub const MAX_ERRORS: usize = 3;
/// The longest quoted error.
pub const MAX_ERROR_CHARS: usize = 140;
/// The longest reason quoted for a rollback that did not happen.
const MAX_REASON_CHARS: usize = 200;

/// What the automatic rollback did, as the check's row records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rollback {
    /// The app does not roll back automatically.
    NotAsked,
    /// Production was rolled back to this build (its publish id).
    RolledBack { to: String },
    /// It was asked for and did not happen, and why.
    NotRolledBack { reason: String },
}

/// A failed production check, as its notice reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductionCheckRecord {
    pub app_name: String,
    pub app_slug: String,
    /// The build that went live: its publish id.
    pub build: String,
    /// The build production served before it, when one is known.
    pub previous: Option<String>,
    /// The first [`MAX_ERRORS`] new errors, each `/<path> at <viewport>: <error>`.
    pub errors: Vec<String>,
    /// How many new errors there are in all.
    pub total: usize,
    /// The attempt had a baseline from before the move, so its errors are
    /// new ones. Without one they are only what production shows.
    pub baselined: bool,
    pub rollback: Rollback,
}

/// `text` cut to `max` characters, marked when cut.
fn cap(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// One `new_errors` entry as a line; `None` for an entry with no error text.
fn error_line(entry: &Value) -> Option<String> {
    let text = |name: &str| entry.get(name).and_then(Value::as_str);
    let error = cap(text("error")?, MAX_ERROR_CHARS);
    let path = text("path").unwrap_or_default();
    match text("viewport") {
        Some(viewport) => Some(format!("/{path} at {viewport}: {error}")),
        None => Some(format!("/{path}: {error}")),
    }
}

/// The rollback a row's `detail.rollback` records.
fn rollback_of(detail: &Value) -> Rollback {
    let Some(recorded) = detail.get("rollback").filter(|r| r.is_object()) else {
        return Rollback::NotAsked;
    };
    let to = recorded
        .pointer("/to/build_id")
        .and_then(Value::as_str)
        .filter(|_| recorded.get("rolled_back") == Some(&Value::Bool(true)));
    match to {
        Some(to) => Rollback::RolledBack { to: to.to_string() },
        None => Rollback::NotRolledBack {
            reason: cap(
                recorded
                    .get("reason")
                    .and_then(Value::as_str)
                    .unwrap_or("the rollback could not be made"),
                MAX_REASON_CHARS,
            ),
        },
    }
}

/// The record of one failed check row, its app and its build.
pub fn record_of(
    check: &app_build_checks::Model,
    app: &apps::Model,
    build: &app_builds::Model,
) -> ProductionCheckRecord {
    let empty = Value::Null;
    let detail = check.detail.as_ref().unwrap_or(&empty);
    let listed: Vec<String> = detail
        .get("new_errors")
        .and_then(Value::as_array)
        .map(|errors| errors.iter().filter_map(error_line).collect())
        .unwrap_or_default();
    let omitted = detail
        .get("new_errors_omitted")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    ProductionCheckRecord {
        app_name: app.name.clone(),
        app_slug: app.slug.clone(),
        build: build.build_id.clone(),
        previous: detail
            .pointer("/move/previous/build_id")
            .and_then(Value::as_str)
            .map(str::to_string),
        total: listed.len() + omitted,
        errors: listed.into_iter().take(MAX_ERRORS).collect(),
        baselined: detail.pointer("/move/baseline") != Some(&Value::Bool(false)),
        rollback: rollback_of(detail),
    }
}

/// The failed checks among `ids` (row ids) that can still be found, each with
/// its app and build: three queries for the page, not three per notice.
pub async fn load(
    db: &DatabaseConnection,
    ids: Vec<String>,
) -> Result<Vec<(String, Record)>, DbErr> {
    let ids: Vec<Uuid> = ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let checks = app_build_checks::Entity::find()
        .filter(app_build_checks::Column::Id.is_in(ids))
        .all(db)
        .await?;
    let app_ids: Vec<Uuid> = checks.iter().map(|c| c.app_id).collect();
    let build_ids: Vec<Uuid> = checks.iter().map(|c| c.build_id).collect();
    let apps: HashMap<Uuid, apps::Model> = apps::Entity::find()
        .filter(apps::Column::Id.is_in(app_ids))
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.id, a))
        .collect();
    let builds: HashMap<Uuid, app_builds::Model> = app_builds::Entity::find()
        .filter(app_builds::Column::Id.is_in(build_ids))
        .all(db)
        .await?
        .into_iter()
        .map(|b| (b.id, b))
        .collect();
    Ok(checks
        .iter()
        .filter_map(|check| {
            let (app, build) = (apps.get(&check.app_id)?, builds.get(&check.build_id)?);
            let record = Record::ProductionCheck(record_of(check, app, build));
            Some((check.id.to_string(), record))
        })
        .collect())
}

/// `3 new errors`, `1 new error`; `3 errors` where none can be called new.
fn counted(total: usize, baselined: bool) -> String {
    let new = if baselined { "new " } else { "" };
    format!("{total} {new}error{}", if total == 1 { "" } else { "s" })
}

/// The notice's first sentence. After an automatic rollback the build is no
/// longer live, so it is told in the past: "is live and shows" read as the
/// state of production above a last sentence saying it had been rolled back.
fn opening(record: &ProductionCheckRecord) -> String {
    let since = match &record.previous {
        Some(previous) => format!(" since it replaced build {previous}"),
        None => String::new(),
    };
    let errors = counted(record.total, record.baselined);
    match record.rollback {
        Rollback::RolledBack { .. } => {
            format!(
                "Build {} showed {errors} on production{since}.",
                record.build
            )
        }
        _ => format!(
            "Build {} is live and production shows {errors}{since}.",
            record.build
        ),
    }
}

/// What the notice says. `org_slug` places the link in the staff console,
/// on the app's Releases page, where the check and Roll back are.
pub fn words(record: &ProductionCheckRecord, org_slug: Option<&str>) -> Rendered {
    let mut body = opening(record);
    if !record.errors.is_empty() {
        body.push(' ');
        body.push_str(&record.errors.join("; "));
        let more = record.total.saturating_sub(record.errors.len());
        if more > 0 {
            body.push_str(&format!(" (and {more} more)"));
        }
        body.push('.');
    }
    body.push(' ');
    body.push_str(&match &record.rollback {
        Rollback::RolledBack { to } => {
            format!("Production was rolled back to build {to} automatically.")
        }
        Rollback::NotRolledBack { reason } => {
            format!("Production was not rolled back automatically: {reason}.")
        }
        Rollback::NotAsked => "Roll back or fix forward from the app's Releases page.".to_string(),
    });
    Rendered {
        title: format!(
            "{}: {} on production after build {}",
            record.app_name,
            if record.baselined {
                "new errors"
            } else {
                "errors"
            },
            record.build
        ),
        body: Some(body),
        href: org_slug.map(|org| format!("/admin/apps/{org}/{}/releases", record.app_slug)),
    }
}

#[cfg(test)]
#[path = "production_check_tests.rs"]
mod tests;
