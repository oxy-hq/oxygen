//! Task outcome persistence and multi-table transactional helpers for the
//! crash-consistent child → parent handoff.

use sea_orm::sea_query::OnConflict;
use sea_orm::{
    ActiveValue::*, ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
    TransactionTrait,
};
use serde_json::Value;

use crate::lifecycle::crud::now;
use crate::lifecycle::entity::{run, run_suspension};
use crate::orchestrator::entity::task_outcome;

/// Atomically record a child task's outcome. This is the **single source of
/// truth** for whether a child has completed and what it returned. Written
/// before the parent's `task_metadata` is updated, so crash recovery can
/// reconstruct fan-out state from this table alone.
///
/// Uses upsert so retries and recovery are idempotent.
pub async fn insert_task_outcome(
    db: &DatabaseConnection,
    child_id: &str,
    parent_id: &str,
    status: &str,
    answer: Option<&str>,
) -> Result<(), DbErr> {
    let model = task_outcome::ActiveModel {
        child_id: Set(child_id.to_string()),
        parent_id: Set(parent_id.to_string()),
        status: Set(status.to_string()),
        answer: Set(answer.map(ToString::to_string)),
        created_at: Set(now()),
    };
    match task_outcome::Entity::insert(model)
        .on_conflict(
            OnConflict::column(task_outcome::Column::ChildId)
                .update_columns([task_outcome::Column::Status, task_outcome::Column::Answer])
                .to_owned(),
        )
        .exec(db)
        .await
    {
        Ok(_) | Err(DbErr::RecordNotInserted) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Atomically mark a child run as done AND record its outcome in a single
/// transaction. Combines `transition_run(done)` + `insert_task_outcome(done)`.
pub async fn complete_child_done_txn(
    db: &DatabaseConnection,
    child_run_id: &str,
    child_task_id: &str,
    parent_id: &str,
    answer: &str,
) -> Result<(), DbErr> {
    let ts = now();
    let txn = db.begin().await?;

    // 1. Update child run to done.
    let model = run::ActiveModel {
        id: Set(child_run_id.to_string()),
        task_status: Set(Some("done".to_string())),
        answer: Set(Some(answer.to_string())),
        updated_at: Set(ts),
        ..Default::default()
    };
    run::Entity::update(model).exec(&txn).await?;

    // 2. Insert task outcome (crash-consistent handoff).
    let outcome = task_outcome::ActiveModel {
        child_id: Set(child_task_id.to_string()),
        parent_id: Set(parent_id.to_string()),
        status: Set("done".to_string()),
        answer: Set(Some(answer.to_string())),
        created_at: Set(ts),
    };
    match task_outcome::Entity::insert(outcome)
        .on_conflict(
            OnConflict::column(task_outcome::Column::ChildId)
                .update_columns([task_outcome::Column::Status, task_outcome::Column::Answer])
                .to_owned(),
        )
        .exec(&txn)
        .await
    {
        Ok(_) | Err(DbErr::RecordNotInserted) => {}
        Err(e) => {
            txn.rollback().await.ok();
            return Err(e);
        }
    }

    txn.commit().await
}

/// Atomically mark a child run as failed/cancelled AND record its outcome in a
/// single transaction. Combines `transition_run(status)` + `insert_task_outcome`.
pub async fn complete_child_failed_txn(
    db: &DatabaseConnection,
    child_run_id: &str,
    child_task_id: &str,
    parent_id: &str,
    status: &str,
    error_message: &str,
) -> Result<(), DbErr> {
    let ts = now();
    let txn = db.begin().await?;

    // 1. Update child run to failed/cancelled.
    let model = run::ActiveModel {
        id: Set(child_run_id.to_string()),
        task_status: Set(Some(status.to_string())),
        error_message: Set(Some(error_message.to_string())),
        updated_at: Set(ts),
        ..Default::default()
    };
    run::Entity::update(model).exec(&txn).await?;

    // 2. Insert task outcome (crash-consistent handoff).
    let outcome = task_outcome::ActiveModel {
        child_id: Set(child_task_id.to_string()),
        parent_id: Set(parent_id.to_string()),
        status: Set(status.to_string()),
        answer: Set(Some(error_message.to_string())),
        created_at: Set(ts),
    };
    match task_outcome::Entity::insert(outcome)
        .on_conflict(
            OnConflict::column(task_outcome::Column::ChildId)
                .update_columns([task_outcome::Column::Status, task_outcome::Column::Answer])
                .to_owned(),
        )
        .exec(&txn)
        .await
    {
        Ok(_) | Err(DbErr::RecordNotInserted) => {}
        Err(e) => {
            txn.rollback().await.ok();
            return Err(e);
        }
    }

    txn.commit().await
}

/// Atomically update a run's status AND persist suspension data in a single
/// transaction. Combines `transition_run` + `upsert_suspension`.
pub async fn suspend_with_data_txn(
    db: &DatabaseConnection,
    run_id: &str,
    task_status: &str,
    task_metadata: Option<Value>,
    prompt: &str,
    suggestions: &[String],
    resume_data: &agentic_core::human_input::SuspendedRunData,
) -> Result<(), DbErr> {
    let ts = now();
    let txn = db.begin().await?;

    // 1. Update run status + metadata.
    update_task_status_merging_metadata(&txn, run_id, task_status, task_metadata).await?;

    // 2. Upsert suspension data.
    let suggestions_val: Value = serde_json::to_value(suggestions).unwrap();
    let resume_val: Value = serde_json::to_value(resume_data).unwrap();
    let suspension = run_suspension::ActiveModel {
        run_id: Set(run_id.to_string()),
        prompt: Set(prompt.to_string()),
        suggestions: Set(suggestions_val),
        resume_data: Set(resume_val),
        created_at: Set(ts),
    };
    run_suspension::Entity::insert(suspension)
        .on_conflict(
            OnConflict::column(run_suspension::Column::RunId)
                .update_columns([
                    run_suspension::Column::Prompt,
                    run_suspension::Column::Suggestions,
                    run_suspension::Column::ResumeData,
                    // Tracks the CURRENT suspension, not the run's first — see
                    // `lifecycle::crud::suspension::upsert_suspension`, which
                    // carries the full reasoning. This is the hot path for
                    // delegation (every suspended step lands here), so it is the
                    // one that must not go stale.
                    run_suspension::Column::CreatedAt,
                ])
                .to_owned(),
        )
        .exec(&txn)
        .await?;

    txn.commit().await
}

/// Set a run's status and **merge** `task_metadata` into what the row has.
///
/// Merge, not replace: a delegated child's row already carries
/// `{policy, original_spec}` from [`insert_child_run`], and when that child
/// delegates in turn its fan-out state lands here. Replacing the column threw
/// the `original_spec` away, and resuming the child is precisely what reads it
/// back — a delegated run has `metadata = None`, so the executor recovers its
/// `agent_id` from `task_metadata.original_spec.agent_id`, found nothing, and
/// failed with "resume: agent_id is empty". A top-level merge (`||`) still
/// overwrites the keys being written, so a second delegation replaces the
/// first one's `child_task_ids` as before. A row whose column is not an
/// object (absent, or JSON `null`) starts from `{}`.
///
/// Every coordinator write of delegation state goes through here: the
/// suspension itself, and each child result folded into `completed`
/// (`Coordinator::record_child_result`) — that one rewrote the whole column too.
pub async fn update_task_status_merging_metadata<C: sea_orm::ConnectionTrait>(
    db: &C,
    run_id: &str,
    task_status: &str,
    task_metadata: Option<Value>,
) -> Result<(), DbErr> {
    use sea_orm::{DatabaseBackend, Statement};
    let sql = "\
        UPDATE agentic_runs SET task_status = $1, updated_at = $2, \
            task_metadata = CASE WHEN $3::jsonb IS NULL THEN task_metadata \
                ELSE (CASE WHEN jsonb_typeof(task_metadata) = 'object' \
                      THEN task_metadata ELSE '{}'::jsonb END) || $3::jsonb END \
        WHERE id = $4";
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [
                task_status.into(),
                now().into(),
                task_metadata.into(),
                run_id.into(),
            ],
        ))
        .await?;
    if result.rows_affected() == 0 {
        // What `Entity::update` reported for a missing row before this was SQL.
        return Err(DbErr::RecordNotUpdated);
    }
    Ok(())
}

/// Create a child run with its initial task_metadata (policy/spec for recovery)
/// in a single INSERT. Combines what was previously `insert_run_with_parent` +
/// `update_task_status(child, "running", metadata)` — two round trips → one.
///
/// The queue entry is NOT included — the caller must still call
/// `transport.assign()` to enqueue the task for worker pickup.
pub async fn insert_child_run(
    db: &DatabaseConnection,
    child_run_id: &str,
    parent_run_id: &str,
    question: &str,
    source_type: &str,
    attempt: i32,
    task_metadata: Option<Value>,
) -> Result<(), DbErr> {
    // Inherit the parent's workspace_id so out-of-process drivers can
    // route every row in a task tree to the same `PlatformContext`. One
    // extra round-trip here on each delegation; coordinator caches this
    // via the `run` model once it's loaded, so the lookup is bounded.
    let parent = run::Entity::find_by_id(parent_run_id.to_string())
        .one(db)
        .await?
        .ok_or_else(|| {
            DbErr::RecordNotFound(format!(
                "insert_child_run: parent run {parent_run_id} not found"
            ))
        })?;
    let workspace_id = parent.workspace_id;

    let ts = now();
    let run_model = run::ActiveModel {
        id: Set(child_run_id.to_string()),
        question: Set(question.to_string()),
        answer: Set(None),
        error_message: Set(None),
        thread_id: Set(None),
        source_type: Set(Some(source_type.to_string())),
        metadata: Set(None),
        parent_run_id: Set(Some(parent_run_id.to_string())),
        schedule_id: Set(None),
        task_status: Set(Some("running".to_string())),
        task_metadata: Set(task_metadata),
        attempt: Set(attempt),
        recovery_requested_at: Set(None),
        driver_id: Set(None),
        driver_heartbeat_at: Set(None),
        cancel_requested_at: Set(None),
        workspace_id: Set(workspace_id),
        created_at: Set(ts),
        updated_at: Set(ts),
    };
    run::Entity::insert(run_model).exec(db).await?;
    Ok(())
}

/// Load all persisted child outcomes for a given parent task.
/// Used by `Coordinator::from_db` to reconstruct `WaitingOnChildren` state
/// without relying on `task_metadata` JSONB.
pub async fn get_outcomes_for_parent(
    db: &DatabaseConnection,
    parent_id: &str,
) -> Result<Vec<task_outcome::Model>, DbErr> {
    task_outcome::Entity::find()
        .filter(task_outcome::Column::ParentId.eq(parent_id))
        .all(db)
        .await
}

/// Get the answer column from a run (used during recovery to fill in
/// outcomes that were persisted on the child but not yet in the outcomes table).
pub async fn get_run_answer(
    db: &DatabaseConnection,
    run_id: &str,
) -> Result<Option<String>, DbErr> {
    let run = run::Entity::find_by_id(run_id.to_string()).one(db).await?;
    Ok(run.and_then(|r| r.answer))
}
