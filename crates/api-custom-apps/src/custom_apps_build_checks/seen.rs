//! `POST …/checks/changes/seen`: a person marks the screens a build changes
//! as seen.
//!
//! `changes` only informs, so it holds a promote one way: where the app's
//! `seen_required` setting is on, a build whose screens differ from
//! production is held (`changes_not_seen`) until someone has looked. This is
//! the look: it writes `detail.seen = {"by": "<who>", "at": "<time>"}` on the
//! attempt's `changes` row, which `custom_apps_promote::checks_gate` reads.
//!
//! **A person's, never the runner's.** The console door (`admit::reader`:
//! non-production reach, never a publish token, never the platform checker,
//! which the console nest's layers refuse before this runs), and a report can
//! never set `seen` (`runner::report::PLATFORM_KEYS`).
//!
//! **Written once.** The first mark stands: marking again answers what the
//! row holds and writes nothing, no second audit row either. Audited
//! `app.build_changes_seen`.
//!
//! `409 nothing_to_see` when there is nothing to mark: the attempt has no
//! `changes` check, the comparison has not finished or did not run, or no
//! screen changed.

use axum::Json;
use axum::extract::Path;
use chrono::{SecondsFormat, Utc};
use entity::app_build_checks;
use oxy::database::client::establish_connection;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::admit;
use super::environment::CheckEnvironment;
use super::error::{ChecksError, internal};
use super::kind::{CheckKind, CheckStatus};
use super::runner::attempt_of;
use super::store;
use crate::admin::apps::environment_scope::Caller;

#[derive(Debug, Default, Deserialize)]
pub struct SeenRequest {
    /// The staging attempt; the newest when absent or null.
    #[serde(default)]
    pub attempt: Option<i32>,
}

/// Who marked the changed screens seen, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub by: String,
    pub at: String,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SeenAnswer {
    pub attempt: i32,
    /// How many screens differ from production, of how many compared.
    pub changed: u64,
    pub compared: u64,
    pub seen: Seen,
}

fn nothing_to_see(why: &str) -> ChecksError {
    ChecksError::conflict("nothing_to_see", format!("nothing to mark as seen: {why}"))
}

/// `(changed, compared)` of a `changes` row that has screens to look at, or
/// why it has none.
pub fn to_see(row: &app_build_checks::Model) -> Result<(u64, u64), ChecksError> {
    if row.status != CheckStatus::Passed.as_str() {
        return Err(nothing_to_see(&format!(
            "the comparison with production is `{}`, so it has no screens to look at",
            row.status
        )));
    }
    let count = |name: &str| {
        let detail = row.detail.as_ref();
        detail.and_then(|d| d.get(name)).and_then(Value::as_u64)
    };
    let (changed, compared) = (
        count("changed").unwrap_or(0),
        count("compared").unwrap_or(0),
    );
    if changed == 0 {
        return Err(nothing_to_see("no screen differs from production"));
    }
    Ok((changed, compared))
}

/// The mark a row already holds.
fn seen_of(row: &app_build_checks::Model) -> Option<Seen> {
    let stored = row.detail.as_ref()?.get("seen")?.clone();
    serde_json::from_value(stored).ok()
}

/// The `changes` row of `key`'s attempt; not-found for an attempt that does
/// not exist, `nothing_to_see` for one with no `changes` check.
async fn changes_row(
    db: &DatabaseConnection,
    key: &store::AttemptKey,
) -> Result<app_build_checks::Model, ChecksError> {
    let rows = store::rows_of(db, key).await?;
    if rows.is_empty() {
        return Err(ChecksError::NotFound);
    }
    rows.into_iter()
        .find(|row| row.kind == CheckKind::Changes.as_str())
        .ok_or_else(|| nothing_to_see("this attempt did not compare the build with production"))
}

/// Write `by`'s mark on `row` unless it already holds one. The mark the row
/// holds afterwards, and whether this call wrote it.
async fn write_mark(
    db: &DatabaseConnection,
    key: &store::AttemptKey,
    row: &app_build_checks::Model,
    by: &str,
) -> Result<(Seen, bool), ChecksError> {
    if let Some(already) = seen_of(row) {
        return Ok((already, false));
    }
    let seen = Seen {
        by: by.to_string(),
        at: Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true),
    };
    let mark = json!({ "by": seen.by, "at": seen.at });
    if store::mark_seen(db, row.id, mark).await? {
        return Ok((seen, true));
    }
    // Someone marked it between the read and the write: theirs stands.
    let theirs = seen_of(&changes_row(db, key).await?);
    let theirs = theirs.ok_or_else(|| internal("the changes row took no mark and holds none"))?;
    Ok((theirs, false))
}

/// `POST /api/customer-apps/{id}/builds/{build}/checks/changes/seen`.
pub async fn mark(
    _: RefuseSandboxAgent,
    actor: RequestActor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path((id, build)): Path<(Uuid, String)>,
    Json(request): Json<SeenRequest>,
) -> Result<Json<SeenAnswer>, ChecksError> {
    let db = establish_connection().await.map_err(internal)?;
    let caller = Caller::new(&actor.user, marker.as_ref());
    let (app, build) = admit::reader(&db, &caller, id, &build).await?;
    let key = attempt_of(&db, &build, CheckEnvironment::Staging, request.attempt).await?;
    let row = changes_row(&db, &key).await?;
    let (changed, compared) = to_see(&row)?;
    let (seen, wrote) = write_mark(&db, &key, &row, actor.user.label()).await?;
    if wrote {
        let entry = AuditEntry::for_request(&actor, "app.build_changes_seen")
            .org(app.org_id)
            .workspace(app.project_id)
            .target("custom_app", app.id.to_string(), app.slug.clone())
            .metadata(json!({ "build": build.build_id, "build_pk": build.id,
                "environment": key.environment, "attempt": key.attempt,
                "changed": changed, "compared": compared }));
        audit::record_best_effort(&db, entry).await;
    }
    Ok(Json(SeenAnswer {
        attempt: key.attempt,
        changed,
        compared,
        seen,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn changes(status: &str, detail: Value) -> app_build_checks::Model {
        app_build_checks::Model {
            id: Uuid::from_u128(1),
            app_id: Uuid::from_u128(2),
            build_id: Uuid::from_u128(3),
            environment: "staging".into(),
            kind: "changes".into(),
            attempt: 1,
            status: status.into(),
            advisory: true,
            summary: None,
            detail: Some(detail),
            notes: json!([]),
            run_id: None,
            runner: None,
            created_at: Default::default(),
            started_at: None,
            finished_at: None,
        }
    }

    #[test]
    fn there_is_something_to_see_only_when_a_finished_comparison_found_a_change() {
        let differ = json!({ "findings": [], "compared": 28, "changed": 4, "seen": null });
        assert_eq!(to_see(&changes("passed", differ.clone())).unwrap(), (4, 28));
        let same = json!({ "findings": [], "compared": 28, "changed": 0, "seen": null });
        for (status, detail) in [
            ("passed", same),
            ("passed", json!({ "findings": [] })),
            ("skipped", differ.clone()),
            ("running", differ.clone()),
            ("error", differ.clone()),
        ] {
            let refused = to_see(&changes(status, detail)).unwrap_err();
            assert_eq!(refused.code(), Some("nothing_to_see"), "{status}");
        }
    }

    #[test]
    fn a_mark_is_read_back_and_null_is_no_mark() {
        let unseen = changes("passed", json!({ "findings": [], "seen": null }));
        assert_eq!(seen_of(&unseen), None);
        let marked = changes(
            "passed",
            json!({ "findings": [], "seen": { "by": "ana@oxy.tech", "at": "2026-11-22T10:00:00Z" } }),
        );
        assert_eq!(seen_of(&marked).unwrap().by, "ana@oxy.tech");
    }
}
