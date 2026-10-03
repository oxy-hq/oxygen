//! `/admin/internal-jobs/recent-failures` and `/dead-letter/*` — the failure feed,
//! the dead-letter list, and the two operator actions on a dead task (re-enqueue,
//! delete).
//!
//! Split out of `internal_jobs.rs` by responsibility; the routes are mounted there.
//! Every list and count is narrowed, and every action fenced, by
//! `internal_jobs_reach`.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::Response;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, FromQueryResult, Statement,
};
use serde::{Deserialize, Serialize};

use super::internal_jobs::{connect, db_err, error_body, not_found_row};
use super::internal_jobs_reach::{deny_out_of_scope_task, listing_scope, run_scope_clause};
use super::internal_jobs_rows::{ENRICHED_SELECT, EnrichedQueueRowRaw, QueueRowDto, QueueRowRaw};
use super::scope::org_scope_clause;

// Recent failures

#[derive(Deserialize, Default)]
pub struct LimitQuery {
    pub limit: Option<u64>,
}

pub async fn recent_failures(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(q): Query<LimitQuery>,
) -> Result<Json<Vec<QueueRowDto>>, Response> {
    let db = connect().await?;
    let scope = listing_scope(&db, &actor).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500) as i64;
    let mut values: Vec<sea_orm::Value> = vec![limit.into()];
    let in_scope = org_scope_clause("w.org_id", scope.as_deref(), &mut values);
    let sql = format!(
        "{ENRICHED_SELECT} \
         WHERE q.queue_status IN ('failed', 'dead'){in_scope} \
         ORDER BY q.updated_at DESC \
         LIMIT $1"
    );
    let rows = EnrichedQueueRowRaw::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(&db)
    .await
    .map_err(db_err)?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

// Dead-letter list + actions

#[derive(Deserialize, Default)]
pub struct DeadLetterQuery {
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

#[derive(Serialize, Debug)]
pub struct DeadLetterResponse {
    pub rows: Vec<QueueRowDto>,
    pub total: i64,
}

#[derive(Debug, FromQueryResult)]
struct CountRow {
    cnt: i64,
}

pub async fn list_dead_letter(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Query(q): Query<DeadLetterQuery>,
) -> Result<Json<DeadLetterResponse>, Response> {
    let db = connect().await?;
    let scope = listing_scope(&db, &actor).await?;
    let limit = q.limit.unwrap_or(50).clamp(1, 500) as i64;
    let offset = q.offset.unwrap_or(0) as i64;

    // The scope goes into BOTH statements: the page, ahead of LIMIT/OFFSET, and
    // the total, which would otherwise report every tenant's dead-letter depth.
    let mut values: Vec<sea_orm::Value> = vec![limit.into(), offset.into()];
    let in_scope = org_scope_clause("w.org_id", scope.as_deref(), &mut values);
    let sql = format!(
        "{ENRICHED_SELECT} \
         WHERE q.queue_status = 'dead'{in_scope} \
         ORDER BY q.updated_at DESC \
         LIMIT $1 OFFSET $2"
    );
    let rows = EnrichedQueueRowRaw::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(&db)
    .await
    .map_err(db_err)?;

    let mut count_values: Vec<sea_orm::Value> = Vec::new();
    let count_scope = run_scope_clause("run_id", scope.as_deref(), &mut count_values);
    let total = CountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        format!(
            "SELECT COUNT(*) AS cnt FROM agentic_task_queue \
             WHERE queue_status = 'dead'{count_scope}"
        ),
        count_values,
    ))
    .one(&db)
    .await
    .map_err(db_err)?
    .map(|r| r.cnt)
    .unwrap_or(0);

    Ok(Json(DeadLetterResponse {
        rows: rows.into_iter().map(Into::into).collect(),
        total,
    }))
}

#[derive(Debug, FromQueryResult)]
struct StatusOnlyRow {
    queue_status: String,
}

/// Look up the current queue_status for a given task_id. Returns:
/// - `Ok(Some(status))` — row exists
/// - `Ok(None)` — no row
/// - `Err(_)` — DB error
async fn current_status(db: &DatabaseConnection, task_id: &str) -> Result<Option<String>, DbErr> {
    StatusOnlyRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT queue_status FROM agentic_task_queue WHERE task_id = $1",
        [task_id.into()],
    ))
    .one(db)
    .await
    .map(|opt| opt.map(|r| r.queue_status))
}

pub async fn reenqueue_dead(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(task_id): Path<String>,
) -> Result<Json<QueueRowDto>, Response> {
    let db = connect().await?;
    deny_out_of_scope_task(&db, &actor, &task_id).await?;
    let status = current_status(&db, &task_id).await.map_err(db_err)?;
    match status.as_deref() {
        None => Err(not_found_row()),
        Some("dead") => {
            // A revival is FRESH WORK, so it clears the wait-streak and pulls a
            // future `available_at` back — matching `enqueue_task`,
            // `requeue_task` and `reset_task_to_queued`, the other three doors
            // into `queued`.
            //
            // This door needs it MOST: `defer_task` dead-letters a starved task
            // with a 12h-old `first_deferred_at` AND a future `available_at` in
            // one statement, and reviving `dead` rows is this endpoint's whole
            // purpose. Without both writes the operator's action cannot work
            // under contention — the task goes `dead -> queued -> dead` on its
            // first claim, with one log line and nothing in the UI to explain
            // it.
            //
            // TOCTOU guard: the SELECT above is advisory — the reaper or another
            // admin could mutate the row before our UPDATE lands. The
            // `WHERE queue_status = 'dead'` clause prevents the wrong write,
            // but we must inspect rows_affected to detect the race and
            // surface a 409 rather than a misleading 200.
            let result = db
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "UPDATE agentic_task_queue \
                     SET queue_status = 'queued', \
                         claim_count = 0, \
                         worker_id = NULL, \
                         claimed_at = NULL, \
                         last_heartbeat = NULL, \
                         available_at = LEAST(available_at, now()), \
                         first_deferred_at = NULL, \
                         updated_at = now() \
                     WHERE task_id = $1 AND queue_status = 'dead'",
                    [task_id.clone().into()],
                ))
                .await
                .map_err(db_err)?;

            if result.rows_affected() == 0 {
                return Err(error_body(
                    StatusCode::CONFLICT,
                    "not_dead",
                    Some(format!(
                        "Task '{task_id}' was not in queue_status='dead' at write time; \
                         another process likely moved it. Re-fetch and try again."
                    )),
                ));
            }

            let row = QueueRowRaw::find_by_statement(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT task_id, run_id, queue_status, worker_id, claim_count, max_claims, \
                        last_heartbeat, claimed_at, created_at, updated_at, spec \
                 FROM agentic_task_queue WHERE task_id = $1",
                [task_id.into()],
            ))
            .one(&db)
            .await
            .map_err(db_err)?;
            row.map(|r| Json(r.into())).ok_or_else(not_found_row)
        }
        Some(other) => Err(error_body(
            StatusCode::CONFLICT,
            "not_dead",
            Some(format!(
                "Task is in queue_status='{other}', only 'dead' rows can be re-enqueued."
            )),
        )),
    }
}

pub async fn delete_dead(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(task_id): Path<String>,
) -> Result<StatusCode, Response> {
    let db = connect().await?;
    deny_out_of_scope_task(&db, &actor, &task_id).await?;
    let status = current_status(&db, &task_id).await.map_err(db_err)?;
    match status.as_deref() {
        None => Err(not_found_row()),
        Some("dead") => {
            // Same TOCTOU guard as reenqueue_dead.
            let result = db
                .execute_raw(Statement::from_sql_and_values(
                    DatabaseBackend::Postgres,
                    "DELETE FROM agentic_task_queue WHERE task_id = $1 AND queue_status = 'dead'",
                    [task_id.clone().into()],
                ))
                .await
                .map_err(db_err)?;

            if result.rows_affected() == 0 {
                return Err(error_body(
                    StatusCode::CONFLICT,
                    "not_dead",
                    Some(format!(
                        "Task '{task_id}' was not in queue_status='dead' at write time; \
                         another process likely moved it. Re-fetch and try again."
                    )),
                ));
            }

            Ok(StatusCode::NO_CONTENT)
        }
        Some(other) => Err(error_body(
            StatusCode::CONFLICT,
            "not_dead",
            Some(format!(
                "Task is in queue_status='{other}', only 'dead' rows can be deleted."
            )),
        )),
    }
}
