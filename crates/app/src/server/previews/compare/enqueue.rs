//! Queueing a compare under each finished transform build, and the compare
//! row's own state. Row writes and a queue insert only; the compare itself is
//! worker-fleet work.

use agentic_core::delegation::{BackoffStrategy, RetryPolicy, TaskPolicy, TaskSpec};
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, QueryResult,
    Statement, TransactionTrait,
};
use serde_json::json;
use uuid::Uuid;

use super::PREVIEW_COMPARE_KIND;

/// Builds per pass; the next pass continues.
const BUILDS_PER_PASS: i64 = 50;

/// Transform builds whose run is `done` and which have no compare yet.
const DUE_SQL: &str = "\
    SELECT b.run_id FROM workspace_preview_runs b JOIN agentic_runs r ON r.id = b.run_id \
    WHERE b.kind = 'transform_build' AND r.task_status = 'done' \
      AND NOT EXISTS (SELECT 1 FROM workspace_preview_runs c \
                      WHERE c.kind = 'compare' AND c.parent_run_id = b.run_id) \
    ORDER BY b.created_at LIMIT $1";

/// Queue a compare under every finished transform build that has none, at
/// most one per build: the build's row is locked and re-checked in the same
/// transaction that queues it. Returns the compare runs queued.
pub async fn enqueue_compares(db: &DatabaseConnection) -> Result<Vec<String>, DbErr> {
    let due = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            DUE_SQL,
            [BUILDS_PER_PASS.into()],
        ))
        .await?;
    let mut queued = Vec::new();
    for row in due {
        let build: String = row.try_get("", "run_id")?;
        if let Some(run_id) = enqueue_one(db, &build).await? {
            queued.push(run_id);
        }
    }
    Ok(queued)
}

/// Lock the build, check it still has no compare, and queue one — all in one
/// transaction.
async fn enqueue_one(db: &DatabaseConnection, build: &str) -> Result<Option<String>, DbErr> {
    let txn = db.begin().await?;
    let Some(b) = lock_uncompared(&txn, build).await? else {
        txn.rollback().await?;
        return Ok(None);
    };
    let run_id = Uuid::new_v4().to_string();
    let workspace_id: Uuid = b.try_get("", "workspace_id")?;
    insert_row(&txn, &run_id, build, &b).await?;
    seed(&txn, &run_id, build, &b).await?;
    txn.commit().await?;
    tracing::info!(target: "preview", %workspace_id, %run_id, build, "preview compare queued");
    Ok(Some(run_id))
}

/// The build's row, locked, when it has no compare yet. The check is a
/// statement of its own after the lock, so it sees a compare a concurrent
/// sweep committed while this one waited.
async fn lock_uncompared(
    txn: &DatabaseTransaction,
    build: &str,
) -> Result<Option<QueryResult>, DbErr> {
    let locked = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT workspace_id, branch, preview_key, revision_id, target_ref, options \
             FROM workspace_preview_runs WHERE run_id = $1 AND kind = 'transform_build' \
             FOR UPDATE",
            [build.into()],
        ))
        .await?;
    let compared = txn
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "SELECT 1 AS n FROM workspace_preview_runs WHERE kind = 'compare' AND parent_run_id = $1",
            [build.into()],
        ))
        .await?;
    Ok(locked.filter(|_| compared.is_none()))
}

/// The compare's `workspace_preview_runs` row, beside its build's.
async fn insert_row(
    txn: &DatabaseTransaction,
    run_id: &str,
    build: &str,
    b: &QueryResult,
) -> Result<(), DbErr> {
    txn.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "INSERT INTO workspace_preview_runs \
             (run_id, workspace_id, branch, preview_key, revision_id, kind, target_ref, \
              parent_run_id, options, state) \
         VALUES ($1, $2, $3, $4, $5, 'compare', $6, $7, $8, 'queued')",
        [
            run_id.into(),
            b.try_get::<Uuid>("", "workspace_id")?.into(),
            b.try_get::<String>("", "branch")?.into(),
            b.try_get::<String>("", "preview_key")?.into(),
            b.try_get::<Uuid>("", "revision_id")?.into(),
            b.try_get::<Option<String>>("", "target_ref")?.into(),
            build.into(),
            b.try_get::<serde_json::Value>("", "options")?.into(),
        ],
    ))
    .await
    .map(|_| ())
}

/// The compare's `agentic_runs` row and its queued task.
async fn seed(
    txn: &DatabaseTransaction,
    run_id: &str,
    build: &str,
    b: &QueryResult,
) -> Result<(), DbErr> {
    let workspace_id: Uuid = b.try_get("", "workspace_id")?;
    let branch: String = b.try_get("", "branch")?;
    let target_ref: Option<String> = b.try_get("", "target_ref")?;
    let metadata = json!({ "branch": branch, "build_run_id": build, "target_ref": target_ref });
    agentic_runtime::crud::insert_run(
        txn,
        run_id,
        &format!("Compare a preview build with live: {branch}"),
        None,
        PREVIEW_COMPARE_KIND,
        Some(metadata),
        workspace_id,
    )
    .await?;
    agentic_runtime::crud::enqueue_task(
        txn,
        run_id,
        run_id,
        None,
        &TaskSpec::Custom {
            kind: PREVIEW_COMPARE_KIND.to_string(),
            payload: json!({ "preview_run_id": run_id }),
        },
        Some(&retry_policy()),
        TaskScope::Global,
    )
    .await
}

/// A compare fails only when a statement or the database does; a short retry
/// covers the transient case.
fn retry_policy() -> TaskPolicy {
    TaskPolicy {
        retry: Some(RetryPolicy {
            max_retries: 2,
            backoff: BackoffStrategy::Exponential {
                initial_delay_ms: 5_000,
                max_delay_ms: 60_000,
            },
            retry_on: Vec::new(),
        }),
        fallback_targets: Vec::new(),
    }
}

/// The worker picked the compare up. Best-effort, as the row only reports.
pub(super) async fn mark_running(db: &DatabaseConnection, run_id: &str) {
    set_state(
        db,
        "UPDATE workspace_preview_runs \
         SET state = 'running', started_at = now(), finished_at = NULL \
         WHERE run_id = $1 AND kind = 'compare'",
        run_id,
    )
    .await;
}

/// The attempt ended; `agentic_runs` holds how.
pub(super) async fn mark_finished(db: &DatabaseConnection, run_id: &str) {
    set_state(
        db,
        "UPDATE workspace_preview_runs SET state = 'finished', finished_at = now() \
         WHERE run_id = $1 AND kind = 'compare'",
        run_id,
    )
    .await;
}

async fn set_state<C: ConnectionTrait>(db: &C, sql: &str, run_id: &str) {
    let done = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [run_id.into()],
        ))
        .await;
    if let Err(e) = done {
        tracing::warn!(target: "preview", %run_id, error = %e, "could not record a compare's state");
    }
}
