//! Queueing the change check, and the analyze row's own state.
//!
//! One check per staging revision, whoever asks first: the compile worker when
//! a previewed branch's staging compile lands `Ready`, or a create/refresh that
//! reused a revision already ready. `idx_workspace_preview_runs_one_analyze`
//! decides the race; only its winner seeds the `agentic_runs` row and queues the
//! task, in the same transaction, so a row never exists without its task.
//!
//! A check that ended failed or cancelled is not final: the next caller for
//! that revision (a refresh, say) re-queues the same run in place.

use agentic_core::delegation::{BackoffStrategy, RetryPolicy, TaskPolicy, TaskSpec};
use agentic_runtime::orchestrator::crud::queue::TaskScope;
use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use uuid::Uuid;

use super::PREVIEW_ANALYZE_KIND;
use crate::server::previews::namespace::preview_key;

const INSERT_SQL: &str = "\
    INSERT INTO workspace_preview_runs \
        (run_id, workspace_id, branch, preview_key, revision_id, kind, state) \
    VALUES ($1, $2, $3, $4, $5, 'analyze', 'queued') \
    ON CONFLICT (workspace_id, revision_id) WHERE kind = 'analyze' DO NOTHING \
    RETURNING run_id";

/// Flip the revision's check back to `running` when it ended failed or
/// cancelled, clearing the attempt's error and driver lease (what
/// `reset_run_for_retry` does). Conditional on the terminal status, so of two
/// concurrent callers exactly one wins: the row lock makes the second re-check
/// `task_status` and match nothing. A task row still `queued` or `claimed`
/// means a worker is (or will be) driving it, so it is left alone.
const REQUEUE_SQL: &str = "\
    UPDATE agentic_runs a SET task_status = 'running', error_message = NULL, answer = NULL, \
        driver_id = NULL, driver_heartbeat_at = NULL, attempt = 0, updated_at = now() \
    FROM workspace_preview_runs r \
    WHERE r.workspace_id = $1 AND r.revision_id = $2 AND r.kind = 'analyze' \
      AND a.id = r.run_id \
      AND a.task_status IN ('failed', 'cancelled', 'timed_out') \
      AND NOT EXISTS (SELECT 1 FROM agentic_task_queue q \
                      WHERE q.task_id = r.run_id AND q.queue_status IN ('queued', 'claimed')) \
    RETURNING a.id";

/// Queue the change check of `revision_id` unless one is already queued,
/// running or done. `Some(run_id)` when this call queued it (or re-queued a
/// failed one), `None` when another caller had.
pub async fn ensure_enqueued(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: &str,
    revision_id: Uuid,
) -> Result<Option<String>, DbErr> {
    let txn = db.begin().await?;
    let queued = match insert_row(&txn, workspace_id, branch, revision_id).await? {
        Some(run_id) => {
            seed_run(&txn, &run_id, workspace_id, branch, revision_id).await?;
            Some(run_id)
        }
        None => requeue_failed(&txn, workspace_id, revision_id).await?,
    };
    let Some(run_id) = queued else {
        txn.rollback().await?;
        return Ok(None);
    };
    agentic_runtime::crud::enqueue_task(
        &txn,
        &run_id,
        &run_id,
        None,
        &TaskSpec::Custom {
            kind: PREVIEW_ANALYZE_KIND.to_string(),
            payload: serde_json::json!({ "preview_run_id": run_id }),
        },
        Some(&retry_policy()),
        TaskScope::Global,
    )
    .await?;
    txn.commit().await?;
    tracing::info!(%workspace_id, %branch, %revision_id, %run_id, "previews: Airway change check queued");
    Ok(Some(run_id))
}

/// The analyze row, unless the revision already has one.
async fn insert_row<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    branch: &str,
    revision_id: Uuid,
) -> Result<Option<String>, DbErr> {
    let run_id = Uuid::new_v4().to_string();
    let won = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            INSERT_SQL,
            [
                run_id.clone().into(),
                workspace_id.into(),
                branch.into(),
                preview_key(workspace_id, branch).into(),
                revision_id.into(),
            ],
        ))
        .await?
        .is_some();
    Ok(won.then_some(run_id))
}

async fn seed_run<C: ConnectionTrait>(
    db: &C,
    run_id: &str,
    workspace_id: Uuid,
    branch: &str,
    revision_id: Uuid,
) -> Result<(), DbErr> {
    agentic_runtime::crud::insert_run(
        db,
        run_id,
        &format!("Airway change check: {branch}"),
        None,
        PREVIEW_ANALYZE_KIND,
        Some(serde_json::json!({
            "branch": branch,
            "revision_id": revision_id,
        })),
        workspace_id,
    )
    .await
}

/// The revision's existing check, reset to run again when it ended failed or
/// cancelled: the run back to `running` ([`REQUEUE_SQL`]), the old attempt's
/// events dropped, and the analyze row back to `queued`. The caller re-queues
/// the task, which upserts by task id.
async fn requeue_failed<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    revision_id: Uuid,
) -> Result<Option<String>, DbErr> {
    let Some(row) = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            REQUEUE_SQL,
            [workspace_id.into(), revision_id.into()],
        ))
        .await?
    else {
        return Ok(None);
    };
    let run_id: String = row.try_get("", "id")?;
    for sql in [
        "DELETE FROM agentic_run_events WHERE run_id = $1",
        "UPDATE workspace_preview_runs \
         SET state = 'queued', started_at = NULL, finished_at = NULL WHERE run_id = $1",
    ] {
        set_state(db, sql, &run_id).await?;
    }
    tracing::info!(%workspace_id, %revision_id, %run_id, "previews: re-queueing a failed Airway change check");
    Ok(Some(run_id))
}

/// The check fails only on a database error (an unreachable Airhouse or an
/// unbuildable connector is a finding, not a failure), so a short retry covers
/// the transient case.
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

/// After a `Ready` staging compile: queue the check when staff preview that
/// branch at that commit. Best-effort — a compile never fails over it, and a
/// refresh of the preview asks again.
pub async fn enqueue_after_staging_compile(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    branch: Option<&str>,
    git_sha: Option<&str>,
    revision_id: Uuid,
) {
    let Some(branch) = branch else {
        return;
    };
    let result = async {
        let Some(preview) = crate::server::previews::store::find(db, workspace_id, branch).await?
        else {
            return Ok(None);
        };
        // A compile of a commit the preview has since moved off is not the
        // preview's revision; the newer compile will ask for its own check.
        if git_sha.is_some_and(|sha| sha != preview.git_sha) {
            return Ok(None);
        }
        ensure_enqueued(db, workspace_id, branch, revision_id).await
    }
    .await;
    if let Err(e) = result {
        tracing::warn!(%workspace_id, %branch, %revision_id, error = %e,
            "previews: could not queue the Airway change check; a refresh will ask again");
    }
}

/// The worker picked the check up.
pub(super) async fn mark_running<C: ConnectionTrait>(db: &C, run_id: &str) -> Result<(), DbErr> {
    set_state(
        db,
        "UPDATE workspace_preview_runs \
         SET state = 'running', started_at = now(), finished_at = NULL \
         WHERE run_id = $1 AND kind = 'analyze'",
        run_id,
    )
    .await
}

/// The attempt ended, with a report or with an error. A retry of a failed
/// attempt marks it `running` again; `agentic_runs` is where a failure is read
/// from.
pub(super) async fn mark_finished<C: ConnectionTrait>(db: &C, run_id: &str) -> Result<(), DbErr> {
    set_state(
        db,
        "UPDATE workspace_preview_runs \
         SET state = 'finished', finished_at = now() \
         WHERE run_id = $1 AND kind = 'analyze'",
        run_id,
    )
    .await
}

async fn set_state<C: ConnectionTrait>(db: &C, sql: &str, run_id: &str) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [run_id.into()],
    ))
    .await
    .map(|_| ())
}
