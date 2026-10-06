//! The trail of a workspace's status changes.
//!
//! `workspace_health_state` is the present tense: one row per workspace,
//! overwritten every evaluation. This is the past tense — a row each time an
//! evaluation finds the status different from last time — so the Health tab
//! can say that a workspace was unhealthy for three hours on Tuesday, and that
//! it is the fourth time this month, instead of only "unhealthy since 09:10".
//!
//! Written from the evaluation, read by one admin route. Each row holds the
//! status left, the status entered and the dimensions failing at that moment,
//! as names and statuses. Reason strings are not kept: they can quote a
//! connector's error text, and the ones for the current state are on the state
//! row already.
//!
//! What a row is **not**: a record of every evaluation, or of a dimension
//! joining or leaving the failing set while the status stays the same. A
//! workspace that goes unhealthy on `pipeline` and later picks up `queue`
//! without recovering has one row, naming `pipeline`.

use axum::{
    Json,
    extract::{Path, Query},
    http::StatusCode,
    response::Response,
};
use chrono::{DateTime, Duration, FixedOffset, Utc};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{connect, db_err, error_body, scope};

/// Rows older than this are deleted by the evaluation that writes a new one —
/// all but the newest of them, which says what the kept period opened in.
pub const RETENTION_DAYS: i64 = 90;
const DEFAULT_DAYS: i64 = 30;
/// The most transitions one read returns. A workspace that flaps hourly for a
/// month makes ~1,400; past this the response says it was cut.
pub const MAX_TRANSITIONS: usize = 200;

/// Record a status change, if this evaluation is one.
///
/// The first evaluation of a workspace counts (`prev` is `None`): it is where
/// the history starts. Anything else is recorded only when the status differs,
/// so a workspace that stays put writes nothing however often it is checked.
/// Returns whether a row was written.
///
/// **"Differs" is measured against the trail's own last row**, and against
/// `prev` — the state row's status — only while the trail is empty. The state
/// row is written by a separate statement after this one and either can fail
/// alone, so the two can disagree. Trusting `prev` would then write the same
/// change twice (the state write failed, so the next pass finds it again), or
/// leave the trail ending in a status the workspace has left (this write
/// failed, and the next pass sees nothing new). Measured against itself the
/// trail does neither: a repeat is skipped and a missed change is written by
/// the next evaluation, dated late rather than lost.
///
/// Statuses are the stored spellings (`healthy` / `degraded` / `unhealthy`) and
/// `failures` the evaluator's `[{dimension, status}]`, taken as they are
/// written rather than as the evaluator's types so this can be driven — and
/// tested — against a database on its own.
pub async fn record(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    prev: Option<&str>,
    next: &str,
    failures: serde_json::Value,
    now: DateTime<Utc>,
) -> Result<bool, DbErr> {
    let recorded = match last_status(db, workspace_id).await? {
        Some(last) => Some(last),
        None => prev.map(str::to_string),
    };
    if recorded.as_deref() == Some(next) {
        return Ok(false);
    }
    let now = now.fixed_offset();
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO workspace_health_transitions \
           (workspace_id, at, from_status, to_status, failures) \
         VALUES ($1, $2, $3, $4, $5)",
        [
            workspace_id.into(),
            now.into(),
            recorded.into(),
            next.into(),
            failures.into(),
        ],
    ))
    .await?;
    // Bounded here rather than by a sweeper: a workspace only grows this table
    // by changing status, and that is the moment it is already being written.
    //
    // The newest row past the cutoff stays. It is the status the workspace was
    // in when the kept period began and the only record of it: without it, a
    // workspace unhealthy for four months that recovers today would have a
    // history that starts at the recovery and reads as healthy all along.
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "DELETE FROM workspace_health_transitions \
         WHERE workspace_id = $1 AND at < $2 \
           AND id <> (SELECT id FROM workspace_health_transitions \
                      WHERE workspace_id = $1 AND at < $2 \
                      ORDER BY at DESC, id DESC LIMIT 1)",
        [
            workspace_id.into(),
            (now - Duration::days(RETENTION_DAYS)).into(),
        ],
    ))
    .await?;
    Ok(true)
}

/// The status the trail last recorded this workspace entering.
async fn last_status(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
) -> Result<Option<String>, DbErr> {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT to_status FROM workspace_health_transitions \
         WHERE workspace_id = $1 ORDER BY at DESC, id DESC LIMIT 1",
        [workspace_id.into()],
    ))
    .await?
    .map(|row| row.try_get::<String>("", "to_status"))
    .transpose()
}

/// One status change. `failures` is passed through as stored — a list of
/// `{dimension, status}` — rather than typed, so a row naming a dimension that
/// has since been retired still reads.
#[derive(Debug, Serialize, FromQueryResult, PartialEq)]
pub struct Transition {
    pub at: DateTime<FixedOffset>,
    /// `None` for the first evaluation, and for a state that began before
    /// history was kept.
    pub from_status: Option<String>,
    pub to_status: String,
    pub failures: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct HealthHistory {
    pub window_days: i64,
    /// The changes inside the window, newest first.
    pub transitions: Vec<Transition>,
    /// The last change before the window — the state the workspace was in when
    /// the window opened. Without it the stretch before the first change
    /// inside the window has no status. `None` when `truncated`: a cut list no
    /// longer reaches back to the window's start, so this would be joined to a
    /// change it did not lead to.
    pub opening: Option<Transition>,
    /// More changes happened in the window than `transitions` holds.
    pub truncated: bool,
}

const COLUMNS: &str = "at, from_status, to_status, failures";

/// The last change before `since`: the state a window starting there opens in.
async fn last_before(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    since: DateTime<FixedOffset>,
) -> Result<Option<Transition>, DbErr> {
    Transition::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT {COLUMNS} FROM workspace_health_transitions \
             WHERE workspace_id = $1 AND at < $2 ORDER BY at DESC, id DESC LIMIT 1"
        ),
        [workspace_id.into(), since.into()],
    ))
    .one(db)
    .await
}

/// A workspace's status changes over the last `days`.
pub async fn history_of(
    db: &impl ConnectionTrait,
    workspace_id: Uuid,
    days: i64,
    now: DateTime<Utc>,
) -> Result<HealthHistory, DbErr> {
    let since = (now - Duration::days(days)).fixed_offset();
    // One more than is returned, so a full page can be told from a cut one.
    let mut transitions = Transition::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT {COLUMNS} FROM workspace_health_transitions \
             WHERE workspace_id = $1 AND at >= $2 ORDER BY at DESC, id DESC LIMIT $3"
        ),
        [
            workspace_id.into(),
            since.into(),
            (MAX_TRANSITIONS as i64 + 1).into(),
        ],
    ))
    .all(db)
    .await?;
    let truncated = transitions.len() > MAX_TRANSITIONS;
    transitions.truncate(MAX_TRANSITIONS);

    let opening = if truncated {
        None
    } else {
        last_before(db, workspace_id, since).await?
    };

    Ok(HealthHistory {
        window_days: days,
        transitions,
        opening,
        truncated,
    })
}

#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    days: Option<i64>,
}

/// The window asked for, held to what is kept. Asking for more than
/// [`RETENTION_DAYS`] would return a window whose far end has been deleted and
/// call the gap "no changes".
fn window_days(asked: Option<i64>) -> i64 {
    asked.unwrap_or(DEFAULT_DAYS).clamp(1, RETENTION_DAYS)
}

/// `GET /admin/workspace-health/{workspace_id}/history?days=` — a workspace's
/// status changes, newest first. Postgres only.
///
/// Fenced on the workspace's org like the eval trigger beside it: a bounded
/// grant reads the history of its own orgs' workspaces, and one outside the
/// grant answers the 404 a missing workspace does.
pub async fn workspace_health_history(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<HistoryParams>,
) -> Result<Json<HealthHistory>, Response> {
    let db = connect().await?;
    scope::deny_out_of_scope_for_workspace(&db, &actor, workspace_id)
        .await
        .map_err(|status| match status {
            StatusCode::NOT_FOUND => error_body(status, "workspace_not_found", None),
            other => error_body(other, "scope_unreadable", None),
        })?;
    let history = history_of(&db, workspace_id, window_days(params.days), Utc::now())
        .await
        .map_err(db_err)?;
    Ok(Json(history))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_is_held_to_what_is_kept() {
        assert_eq!(window_days(None), DEFAULT_DAYS);
        assert_eq!(window_days(Some(7)), 7);
        assert_eq!(window_days(Some(0)), 1);
        assert_eq!(window_days(Some(-4)), 1);
        assert_eq!(window_days(Some(365)), RETENTION_DAYS);
    }
}
