//! What every kind of preview run shares: when its `agentic_runs` row counts
//! as over, and cancelling a deleted preview's queued runs.

use sea_orm::{
    ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement, TransactionTrait,
};
use uuid::Uuid;

/// `agentic_runs.task_status` values after which a run does nothing more, as a
/// SQL list. A run in any other status (or none) may still be working.
pub const TERMINAL_RUN_STATUSES: &str = "('done','failed','cancelled','timed_out')";

/// Cancel every queued run of preview `preview_key`: its queue rows stop being
/// claimable, its run ends `cancelled`, and its preview-run row `finished`,
/// all in one transaction. A run already running is left to finish. Returns
/// the cancelled run ids.
///
/// A cancelled change check is not lost: previewing the branch again re-queues
/// it (`analyze::ensure_enqueued` re-queues a `cancelled` check).
pub async fn cancel_queued(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    preview_key: &str,
) -> Result<Vec<String>, DbErr> {
    cancel_queued_where(db, workspace_id, preview_key, None).await
}

/// Cancel the queued runs of preview `preview_key` that read a revision of
/// `git_sha` — the commit a refresh just moved the preview off — unless another
/// live preview is still at that commit (a change check is one per revision,
/// shared by every preview at it). Same effect per run as [`cancel_queued`].
pub async fn cancel_queued_at(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    preview_key: &str,
    git_sha: &str,
) -> Result<Vec<String>, DbErr> {
    cancel_queued_where(db, workspace_id, preview_key, Some(git_sha)).await
}

async fn cancel_queued_where(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    preview_key: &str,
    git_sha: Option<&str>,
) -> Result<Vec<String>, DbErr> {
    let txn = db.begin().await?;
    let rows = txn
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE workspace_preview_runs SET state = 'finished', finished_at = now() \
             WHERE workspace_id = $1 AND preview_key = $2 AND state = 'queued' \
               AND ($3::text IS NULL OR ( \
                   revision_id IN (SELECT revision_id FROM revisions \
                                   WHERE workspace_id = $1 AND git_sha = $3) \
                   AND NOT EXISTS (SELECT 1 FROM workspace_previews p \
                                   WHERE p.workspace_id = $1 AND p.git_sha = $3))) \
             RETURNING run_id",
            [
                workspace_id.into(),
                preview_key.into(),
                git_sha.map(str::to_string).into(),
            ],
        ))
        .await?;
    let run_ids = rows
        .iter()
        .map(|r| r.try_get::<String>("", "run_id"))
        .collect::<Result<Vec<_>, _>>()?;
    for run_id in &run_ids {
        agentic_runtime::crud::cancel_queued_tasks_for_run(&txn, run_id).await?;
        cancel_run(&txn, run_id).await?;
    }
    txn.commit().await?;
    if !run_ids.is_empty() {
        tracing::info!(%workspace_id, %preview_key, ?git_sha, runs = ?run_ids,
            "previews: cancelled queued runs of a deleted or superseded preview revision");
    }
    Ok(run_ids)
}

/// Note on run `run_id`, as it starts, which tables its preview already holds
/// (`options.held_before`, `[[schema, table], …]`, dropped ones left out). A
/// transform build's compare flags those as `preexisting`: an earlier run or
/// build wrote them, and the build may have read that copy instead of live.
pub(super) async fn note_held_before<C: ConnectionTrait>(
    db: &C,
    run_id: &str,
    workspace_id: Uuid,
    preview_key: &str,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE workspace_preview_runs SET options = options || jsonb_build_object( \
             'held_before', COALESCE((SELECT jsonb_agg(jsonb_build_array(t.live_schema, \
                 t.table_name) ORDER BY t.live_schema, t.table_name) \
               FROM workspace_preview_tables t \
               WHERE t.workspace_id = $2 AND t.preview_key = $3 AND t.state <> 'dropped'), \
             '[]'::jsonb)) \
         WHERE run_id = $1",
        [run_id.into(), workspace_id.into(), preview_key.into()],
    ))
    .await
    .map(|_| ())
}

async fn cancel_run<C: ConnectionTrait>(db: &C, run_id: &str) -> Result<(), DbErr> {
    let sql = format!(
        "UPDATE agentic_runs SET task_status = 'cancelled', \
             error_message = COALESCE(error_message, 'the preview was deleted'), \
             driver_id = NULL, driver_heartbeat_at = NULL, updated_at = now() \
         WHERE id = $1 AND (task_status IS NULL OR task_status NOT IN {TERMINAL_RUN_STATUSES})"
    );
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        [run_id.into()],
    ))
    .await
    .map(|_| ())
}
