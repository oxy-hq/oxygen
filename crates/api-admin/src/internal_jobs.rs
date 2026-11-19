//! `/api/admin/internal-jobs/*` — operator dashboard for the agentic task
//! queue and worker fleet. Distinct from the customer-facing
//! Coordinator/Orchestrator UI which surfaces individual analytics runs;
//! this module shows the meta view (workers, queue depth, dead-letter,
//! periodic system jobs).
//!
//! The capability gate (`Action::PlatformOperate`) is named in
//! [`crate::admin_nests`] and layered on by `oxy-app`'s `router::global`. It
//! cannot see grant **scope**, so every handler here narrows by it — see
//! `internal_jobs_reach` for the rule.
//!
//! Database access is on-demand via `oxy::database::client::establish_connection()`
//! — same pattern `billing_service()` uses. We deliberately do not put a
//! pool on `AppState` for this admin-only surface.
//!
//! Split by responsibility across sibling modules, re-exported below so the public
//! paths (`oxy_api_admin::internal_jobs::*`, `oxy-app`'s `admin::internal_jobs::*`
//! until the admin split's PR 7) keep one module:
//!
//! * `internal_jobs_rows` — the queue-row DTO, its raw query shapes, and the secret
//!   redaction applied to every decoded `TaskSpec`;
//! * `internal_jobs_dead_letter` — the failure feed and the dead-letter list and
//!   actions (re-enqueue, delete);
//! * `internal_jobs_fleet` — workers, the scheduled-jobs registry, and the manual
//!   reaper / retention runs;
//! * `internal_jobs_reach` — grant scope for all of the above.
//!
//! This file keeps the router and the queue stats. The connection and error helpers
//! the rest of the admin console reuses went down to `oxy_app_core::admin_db` in the
//! admin split's PR 3.

use axum::Json;
use axum::Router;
use axum::http::StatusCode;
use axum::response::Response;
use axum::routing::{delete, get, post};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement};
use serde::Serialize;
use uuid::Uuid;

pub use super::internal_jobs_dead_letter::{
    DeadLetterQuery, DeadLetterResponse, LimitQuery, delete_dead, list_dead_letter,
    recent_failures, reenqueue_dead,
};
use super::internal_jobs_failure_groups::failure_groups;
pub use super::internal_jobs_failure_groups::{FailureGroupDto, FailureGroupsResponse};
use super::internal_jobs_fleet::list_scheduled;
pub use super::internal_jobs_fleet::{
    RunReaperResponse, RunRetentionResponse, ScheduledJobDto, WorkerDto, WorkersResponse,
    list_workers, run_reaper, run_retention,
};
use super::internal_jobs_reach::{listing_scope, run_scope_clause};
pub use super::internal_jobs_rows::QueueRowDto;
use oxy_app_core::AppState;
use oxy_app_core::admin_db::{connect, db_err, error_body};

/// The internal-jobs routes, relative to `/admin/internal-jobs`. They are
/// flat (no `/internal-jobs/` prefix on each): [`crate::admin_nests`] hands
/// the tree to `oxy-app`, whose `router::global` nests it at
/// `/admin/internal-jobs` behind the staff door, the refusal while acting
/// and `PlatformOperate`, without bringing the rest of `/admin/*` along.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/queue-stats", get(queue_stats))
        .route("/recent-failures", get(recent_failures))
        .route("/failure-groups", get(failure_groups))
        .route("/dead-letter", get(list_dead_letter))
        .route("/dead-letter/{task_id}/reenqueue", post(reenqueue_dead))
        .route("/dead-letter/{task_id}", delete(delete_dead))
        .route("/workers", get(list_workers))
        .route("/scheduled", get(list_scheduled))
        .route("/run-reaper", post(run_reaper))
        .route("/run-retention", post(run_retention))
}

// Queue stats

#[derive(Serialize, Default, Debug, PartialEq)]
pub struct QueueStatusCounts {
    pub queued: i64,
    pub claimed: i64,
    pub completed: i64,
    pub failed: i64,
    pub cancelled: i64,
    pub dead: i64,
}

#[derive(Serialize, Debug)]
pub struct QueueStatsResponse {
    pub last_1h: QueueStatusCounts,
    pub last_24h: QueueStatusCounts,
    pub total: QueueStatusCounts,
}

#[derive(Debug, FromQueryResult)]
struct StatusBucketRow {
    bucket: String,
    queue_status: String,
    cnt: i64,
}

pub async fn queue_stats(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<QueueStatsResponse>, Response> {
    let db = connect().await?;
    let scope = listing_scope(&db, &actor).await?;
    let stats = fetch_queue_stats(&db, scope.as_deref())
        .await
        .map_err(db_err)?;
    Ok(Json(stats))
}

/// `scope` is the caller's grant scope (`None` = unbounded): a bounded grant counts
/// only the tasks of the orgs it names, so the totals leak no other tenant's volume.
pub(crate) async fn fetch_queue_stats(
    db: &DatabaseConnection,
    scope: Option<&[Uuid]>,
) -> Result<QueueStatsResponse, DbErr> {
    // Single grouped query — emits one row per (bucket, status) pair. The
    // CASE expression bucketizes by updated_at age; the WHERE clause keeps
    // total a strict superset of the others.
    let mut values: Vec<sea_orm::Value> = Vec::new();
    let in_scope = run_scope_clause("run_id", scope, &mut values);
    let sql = format!(
        "SELECT bucket, queue_status, COUNT(*) AS cnt FROM ( \
            SELECT \
                CASE \
                    WHEN updated_at > now() - interval '1 hour' THEN '1h' \
                    WHEN updated_at > now() - interval '24 hours' THEN '24h' \
                    ELSE 'older' \
                END AS bucket, \
                queue_status \
            FROM agentic_task_queue \
            WHERE TRUE{in_scope} \
        ) t \
        GROUP BY bucket, queue_status"
    );

    let rows = StatusBucketRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;

    let mut out = QueueStatsResponse {
        last_1h: QueueStatusCounts::default(),
        last_24h: QueueStatusCounts::default(),
        total: QueueStatusCounts::default(),
    };
    for row in rows {
        // total always increments
        accumulate(&mut out.total, &row.queue_status, row.cnt);
        if row.bucket == "1h" {
            accumulate(&mut out.last_1h, &row.queue_status, row.cnt);
            accumulate(&mut out.last_24h, &row.queue_status, row.cnt);
        } else if row.bucket == "24h" {
            accumulate(&mut out.last_24h, &row.queue_status, row.cnt);
        }
    }
    Ok(out)
}

fn accumulate(counts: &mut QueueStatusCounts, status: &str, n: i64) {
    match status {
        "queued" => counts.queued += n,
        "claimed" => counts.claimed += n,
        "completed" => counts.completed += n,
        "failed" => counts.failed += n,
        "cancelled" => counts.cancelled += n,
        "dead" => counts.dead += n,
        _ => {}
    }
}

pub(super) fn not_found_row() -> Response {
    error_body(StatusCode::NOT_FOUND, "task_not_found", None)
}

#[cfg(test)]
#[path = "internal_jobs_tests.rs"]
mod tests;
