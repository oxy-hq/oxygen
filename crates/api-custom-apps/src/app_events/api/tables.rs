//! The table subscriptions of `GET …/events/types` (its `tables` array) and
//! `POST …/{app}/events/resync` (app events design §7).
//!
//! `tables` has one row per function that runs on a table, per environment:
//! where its cursor is, how far behind, and the state a person acts on —
//! `active`, `behind_horizon`, `broken`, or `paused` when the circuit breaker
//! paused the subscription (which shows over the cursor's own state, since a
//! paused function receives nothing whatever the table does). Read from the
//! platform's own rows; nothing here touches Airhouse.

use chrono::{DateTime, Utc};
use entity::apps;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::reply::{ApiError, environment};
use crate::app_events::table_watch::resync::{self, ResyncError};

/// The audit action of a resync.
pub const RESYNCED: &str = "app.events.table_resync";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TableRow {
    /// The table as the function wrote it.
    table: String,
    #[serde(rename = "type")]
    event_type: String,
    environment: String,
    app: String,
    function: String,
    /// `active`, `behind_horizon`, `broken` or `paused`.
    state: String,
    detail: Option<String>,
    /// The cursor: the last snapshot delivered.
    snapshot: Option<i64>,
    /// The newest snapshot the last check saw.
    head: Option<i64>,
    lag: Option<i64>,
    checked_at: Option<DateTime<Utc>>,
    advanced_at: Option<DateTime<Utc>>,
    paused_at: Option<DateTime<Utc>>,
}

const TABLES_SQL: &str = "\
SELECT s.event_type, a.slug AS app, s.function_name, s.environment, s.paused_at, s.paused_reason, \
       c.state, c.state_detail, c.snapshot_id, c.head_snapshot, c.checked_at, c.advanced_at \
  FROM app_event_subscriptions s \
  JOIN apps a ON a.id = s.app_id \
  LEFT JOIN app_event_table_cursors c ON c.workspace_id = s.workspace_id \
       AND c.environment = s.environment AND c.table_ref = substr(s.event_type, 7) \
 WHERE s.org_id = $1 AND s.event_type LIKE 'table:%' \
 ORDER BY s.event_type, a.slug, s.function_name, s.environment";

/// Every table subscription of the org.
pub(super) async fn list(db: &DatabaseConnection, org_id: Uuid) -> Result<Vec<TableRow>, DbErr> {
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            TABLES_SQL,
            [org_id.into()],
        ))
        .await?;
    rows.into_iter()
        .map(|row| {
            let event_type: String = row.try_get("", "event_type")?;
            let paused_at: Option<DateTime<Utc>> = row.try_get("", "paused_at")?;
            let cursor_state: Option<String> = row.try_get("", "state")?;
            let snapshot: Option<i64> = row.try_get("", "snapshot_id")?;
            let head: Option<i64> = row.try_get("", "head_snapshot")?;
            let (state, detail) = match paused_at {
                Some(_) => ("paused".to_string(), row.try_get("", "paused_reason")?),
                None => (
                    cursor_state.unwrap_or_else(|| "active".to_string()),
                    row.try_get("", "state_detail")?,
                ),
            };
            Ok(TableRow {
                table: event_type
                    .strip_prefix("table:")
                    .unwrap_or(&event_type)
                    .to_string(),
                event_type,
                environment: row.try_get("", "environment")?,
                app: row.try_get("", "app")?,
                function: row.try_get("", "function_name")?,
                state,
                detail,
                snapshot,
                head,
                lag: snapshot.zip(head).map(|(s, h)| (h - s).max(0)),
                checked_at: row.try_get("", "checked_at")?,
                advanced_at: row.try_get("", "advanced_at")?,
                paused_at,
            })
        })
        .collect()
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResyncBody {
    pub function: Option<String>,
    pub environment: Option<String>,
    #[serde(default)]
    pub backfill: bool,
    #[serde(default)]
    pub dry_run: bool,
}

/// The refusal for a resync that would move a cursor shared with apps the
/// caller may not manage: both sides are named, and nothing moves.
fn shared_cursor_refusal(refused: &[String], app_slug: &str) -> Result<(), ApiError> {
    if refused.is_empty() {
        return Ok(());
    }
    Err(ApiError::Conflict(
        "shared_cursor",
        format!(
            "the cursor of this table is shared by `{}` and {}, which you may not manage: a \
             resync would make them skip what changed meanwhile. Ask someone who manages them too",
            app_slug,
            refused.join(", ")
        ),
    ))
}

/// The other apps sharing the cursor of the function `body` names: a resync
/// moves the cursor for all of them, so the caller must manage each.
pub(super) async fn sharers_of(
    db: &DatabaseConnection,
    app: &apps::Model,
    body: &ResyncBody,
) -> Result<Vec<apps::Model>, ApiError> {
    let function = body.function.as_deref().map(str::trim).unwrap_or_default();
    let environment = environment(body.environment.as_deref())?.name();
    resync::sharers(db, app, function, &environment)
        .await
        .map_err(|e| ApiError::db("find the apps sharing the cursor", e))
}

/// Resync one function of `app`. Audited unless it is a dry run. `refused`
/// names the apps sharing the cursor that the caller may not manage: any, and
/// nothing moves, since they would silently skip what changed meanwhile.
pub(super) async fn resync(
    db: &DatabaseConnection,
    actor: &RequestActor,
    app: &apps::Model,
    body: &ResyncBody,
    refused: Vec<String>,
) -> Result<resync::Report, ApiError> {
    let function = body
        .function
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .ok_or_else(|| ApiError::BadRequest("missing_field", "`function` is required".into()))?;
    let environment = environment(body.environment.as_deref())?.name();
    shared_cursor_refusal(&refused, &app.slug)?;
    let report = resync::resync(db, app, function, &environment, body.backfill, body.dry_run)
        .await
        .map_err(|e| match e {
            ResyncError::NothingWatched { .. } => {
                ApiError::NotFound("no_table_watch", e.to_string())
            }
            ResyncError::NoAirhouse => ApiError::Conflict("no_airhouse", e.to_string()),
            ResyncError::NoCursor(_) => ApiError::Conflict("no_cursor_yet", e.to_string()),
            ResyncError::Failed(message) => {
                tracing::error!(error = %message, "app events api: resync failed");
                ApiError::Internal("could not resync; retry shortly".into())
            }
        })?;
    if !body.dry_run {
        let entry = AuditEntry::for_request(actor, RESYNCED)
            .org(app.org_id)
            .workspace(app.project_id)
            .target(
                "custom_app_environment",
                format!("{}/{environment}", app.id),
                format!("{}/{environment}", app.slug),
            )
            .environment(environment.clone())
            .metadata(json!({
                "function": function,
                "tables": report.tables.iter().map(|t| &t.table).collect::<Vec<_>>(),
                "backfill": report.backfill,
            }));
        audit::record_best_effort(db, entry).await;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An operator of one of two apps sharing a cursor is refused, with both
    /// apps named; one who manages every sharer is not.
    #[test]
    fn a_resync_across_apps_the_caller_may_not_manage_names_them_all() {
        assert!(shared_cursor_refusal(&[], "locations").is_ok());
        let refused =
            shared_cursor_refusal(&["bookkeeping".into(), "watchlist".into()], "locations")
                .expect_err("refused");
        let ApiError::Conflict(code, message) = refused else {
            panic!("a conflict");
        };
        assert_eq!(code, "shared_cursor");
        for name in ["locations", "bookkeeping", "watchlist"] {
            assert!(message.contains(name), "{message}");
        }
    }
}
