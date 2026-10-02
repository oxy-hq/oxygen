//! Lifecycle CRUD on the `agentic_runs` table.

use sea_orm::{
    ActiveValue::*, ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, Statement,
};
use serde_json::Value;
use uuid::Uuid;

use crate::lifecycle::entity::run;

use super::{DRIVER_LEASE_TTL_SECS, now, transition_run};

// ── Driver lease ────────────────────────────────────────────────────────────
//
// A per-run lease (`driver_id` + `driver_heartbeat_at`) recording which
// driver process/loop is actively driving a run. CAS-acquired with a
// staleness window so a periodic recovery loop cannot double-drive a run a
// live driver already owns. See [`DRIVER_LEASE_TTL_SECS`].

/// Try to acquire (or renew) the driver lease on `run_id` for `driver_id`.
///
/// Succeeds when the run is unleased, already held by `driver_id`
/// (idempotent renew), or the current holder's heartbeat is stale past
/// [`DRIVER_LEASE_TTL_SECS`]. Returns `true` if the lease is now held by
/// `driver_id`, `false` if a different live driver owns it (caller must not
/// drive the run — release any claimed queue task and skip).
pub async fn try_acquire_driver(
    db: &DatabaseConnection,
    run_id: &str,
    driver_id: &str,
) -> Result<bool, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            // Deliberately does NOT touch `updated_at`: the lease is
            // bookkeeping, not run progress. Conflating them would push a
            // stranded run back inside the `find_stuck_runs` grace window
            // on every acquire/heartbeat and mask genuine staleness.
            "UPDATE agentic_runs \
             SET driver_id = $1, driver_heartbeat_at = now() \
             WHERE id = $2 \
               AND (driver_id IS NULL \
                    OR driver_id = $1 \
                    OR driver_heartbeat_at IS NULL \
                    OR driver_heartbeat_at < now() - make_interval(secs => $3))",
            [
                driver_id.into(),
                run_id.into(),
                (DRIVER_LEASE_TTL_SECS as i32).into(),
            ],
        ))
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Heartbeat the driver lease. Returns `true` while `driver_id` still holds
/// the lease; `false` means it was lost (the caller should stop driving).
pub async fn heartbeat_driver(
    db: &DatabaseConnection,
    run_id: &str,
    driver_id: &str,
) -> Result<bool, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            // No `updated_at` bump — see `try_acquire_driver`.
            "UPDATE agentic_runs \
             SET driver_heartbeat_at = now() \
             WHERE id = $1 AND driver_id = $2",
            [run_id.into(), driver_id.into()],
        ))
        .await?;
    Ok(res.rows_affected() == 1)
}

/// Release the driver lease, but only if `driver_id` still owns it (so a
/// driver that lost the lease to a stale-takeover cannot clobber the new
/// holder). Terminal `transition_run` clears the lease unconditionally.
pub async fn release_driver(
    db: &DatabaseConnection,
    run_id: &str,
    driver_id: &str,
) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        // No `updated_at` bump — see `try_acquire_driver`. (Terminal
        // `transition_run` legitimately bumps it; that is a real state
        // change, this is not.)
        "UPDATE agentic_runs \
         SET driver_id = NULL, driver_heartbeat_at = NULL \
         WHERE id = $1 AND driver_id = $2",
        [run_id.into(), driver_id.into()],
    ))
    .await?;
    Ok(())
}

// ── Cross-process cancel (§12 FU4a) ──────────────────────────────────────────

/// Durable, cross-process cancel signal. Set by the HTTP cancel endpoint
/// so a recovered / Global run driven out-of-process can be cancelled —
/// the in-memory watch channel only reaches a same-process coordinator.
/// Idempotent; does not overwrite an earlier request timestamp.
pub async fn request_cancel(db: &DatabaseConnection, run_id: &str) -> Result<(), DbErr> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "UPDATE agentic_runs \
         SET cancel_requested_at = COALESCE(cancel_requested_at, now()) \
         WHERE id = $1",
        [run_id.into()],
    ))
    .await?;
    Ok(())
}

/// Has cancel been requested for this run? Polled by the driver's cancel
/// forwarder so the signal is observed cross-process.
pub async fn is_cancel_requested(db: &DatabaseConnection, run_id: &str) -> Result<bool, DbErr> {
    use sea_orm::{FromQueryResult, Statement};
    #[derive(FromQueryResult)]
    struct R {
        requested: bool,
    }
    let row = R::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT cancel_requested_at IS NOT NULL AS requested \
         FROM agentic_runs WHERE id = $1",
        [run_id.into()],
    ))
    .one(db)
    .await?;
    Ok(row.map(|r| r.requested).unwrap_or(false))
}

pub async fn insert_run(
    db: &impl ConnectionTrait,
    run_id: &str,
    question: &str,
    thread_id: Option<Uuid>,
    source_type: &str,
    metadata: Option<Value>,
    workspace_id: Uuid,
) -> Result<(), DbErr> {
    insert_run_inner(
        db,
        run_id,
        question,
        thread_id,
        source_type,
        metadata,
        None,
        None,
        0,
        workspace_id,
    )
    .await
}

/// Insert a run seeded by a scheduler fire. The only path that should stamp
/// `schedule_id`; everything else goes through [`insert_run`]. Lets per-job
/// run history queries do `WHERE schedule_id = $1` against the new index.
#[allow(clippy::too_many_arguments)]
pub async fn insert_run_with_schedule(
    db: &DatabaseConnection,
    run_id: &str,
    question: &str,
    thread_id: Option<Uuid>,
    source_type: &str,
    metadata: Option<Value>,
    schedule_id: &str,
    workspace_id: Uuid,
) -> Result<(), DbErr> {
    insert_run_inner(
        db,
        run_id,
        question,
        thread_id,
        source_type,
        metadata,
        None,
        Some(schedule_id),
        0,
        workspace_id,
    )
    .await
}

/// Insert a child run with a parent reference for the task tree.
///
/// Child rows always inherit the parent's `workspace_id` — pass the
/// parent's `workspace_id` here so a single coordinator txn doesn't pay
/// an extra round-trip to re-read it.
pub async fn insert_run_with_parent(
    db: &DatabaseConnection,
    run_id: &str,
    parent_run_id: &str,
    question: &str,
    source_type: &str,
    metadata: Option<Value>,
    attempt: i32,
    workspace_id: Uuid,
) -> Result<(), DbErr> {
    insert_run_inner(
        db,
        run_id,
        question,
        None,
        source_type,
        metadata,
        Some(parent_run_id),
        None,
        attempt,
        workspace_id,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn insert_run_inner(
    db: &impl ConnectionTrait,
    run_id: &str,
    question: &str,
    thread_id: Option<Uuid>,
    source_type: &str,
    metadata: Option<Value>,
    parent_run_id: Option<&str>,
    schedule_id: Option<&str>,
    attempt: i32,
    workspace_id: Uuid,
) -> Result<(), DbErr> {
    let ts = now();
    let model = run::ActiveModel {
        id: Set(run_id.to_string()),
        question: Set(question.to_string()),
        answer: Set(None),
        error_message: Set(None),
        thread_id: Set(thread_id),
        source_type: Set(Some(source_type.to_string())),
        metadata: Set(metadata),
        parent_run_id: Set(parent_run_id.map(ToString::to_string)),
        schedule_id: Set(schedule_id.map(ToString::to_string)),
        task_status: Set(Some("running".to_string())),
        task_metadata: Set(None),
        attempt: Set(attempt),
        recovery_requested_at: Set(None),
        driver_id: Set(None),
        driver_heartbeat_at: Set(None),
        cancel_requested_at: Set(None),
        workspace_id: Set(workspace_id),
        created_at: Set(ts),
        updated_at: Set(ts),
    };
    run::Entity::insert(model).exec(db).await?;
    Ok(())
}

// ── Compatibility shims ─────────────────────────────────────────────────────
// These thin wrappers delegate to `transition_run` so that existing callers
// continue to compile without modification.

/// Mark a run done, persisting `metadata_patch` onto `task_metadata`.
///
/// The patch used to be accepted and silently discarded, which made root-task
/// completion the one terminal transition that lost its metadata — the
/// suspension path (`crud::suspend_run_txn`) has always persisted it. Any
/// structured result a root task wanted to carry out (airway load stats,
/// rollup counts) vanished without a trace.
///
/// `None` leaves `task_metadata` untouched, matching [`transition_run`]'s
/// "None means leave unchanged" convention, so callers passing `None` are
/// unaffected.
pub async fn update_run_done(
    db: &DatabaseConnection,
    run_id: &str,
    answer: &str,
    metadata_patch: Option<Value>,
) -> Result<(), DbErr> {
    transition_run(db, run_id, "done", metadata_patch, Some(answer), None).await
}

pub async fn update_run_failed(
    db: &DatabaseConnection,
    run_id: &str,
    error: &str,
) -> Result<(), DbErr> {
    transition_run(db, run_id, "failed", None, None, Some(error)).await
}

pub async fn update_run_suspended(db: &DatabaseConnection, run_id: &str) -> Result<(), DbErr> {
    transition_run(db, run_id, "awaiting_input", None, None, None).await
}

pub async fn update_run_running(db: &DatabaseConnection, run_id: &str) -> Result<(), DbErr> {
    transition_run(db, run_id, "running", None, None, None).await
}

/// Persist a coordinator task status transition.
pub async fn update_task_status(
    db: &DatabaseConnection,
    run_id: &str,
    task_status: &str,
    task_metadata: Option<Value>,
) -> Result<(), DbErr> {
    transition_run(db, run_id, task_status, task_metadata, None, None).await
}

/// Load all runs in a task tree (root + descendants) by following `parent_run_id`.
///
/// **Trusted/internal use only.** This loader has no workspace gate —
/// it's used by the runtime recovery loop, which picks up rows it has
/// already validated from the DB and just needs to reconstruct the
/// task tree around them. For HTTP request handling use
/// [`load_task_tree_in_workspace`] instead so a foreign run id can't
/// probe another tenant's tree.
pub async fn load_task_tree(
    db: &DatabaseConnection,
    root_run_id: &str,
) -> Result<Vec<run::Model>, DbErr> {
    let root = run::Entity::find_by_id(root_run_id.to_string())
        .one(db)
        .await?;
    let Some(root) = root else {
        return Ok(vec![]);
    };

    // BFS to collect all descendants.
    let mut result = vec![root];
    let mut parent_ids = vec![root_run_id.to_string()];

    while !parent_ids.is_empty() {
        let children = run::Entity::find()
            .filter(run::Column::ParentRunId.is_in(&parent_ids))
            .all(db)
            .await?;
        parent_ids = children.iter().map(|c| c.id.clone()).collect();
        result.extend(children);
    }

    Ok(result)
}

/// Workspace-scoped variant of [`load_task_tree`] for HTTP handlers.
///
/// Returns an empty Vec if the root run doesn't belong to
/// `workspace_id`, so a foreign run id can't probe another tenant's
/// tree by id-guessing — and likewise if it is a check run queued outside
/// production, which is staff's to read (`visibility::get_run_in_workspace`).
/// Children inherit the parent's workspace_id at insert so the BFS doesn't
/// need an additional filter — the root gate prevents traversal from
/// escaping the workspace.
pub async fn load_task_tree_in_workspace(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    root_run_id: &str,
) -> Result<Vec<run::Model>, DbErr> {
    let root = super::visibility::get_run_in_workspace(db, workspace_id, root_run_id).await?;
    let Some(root) = root else {
        return Ok(vec![]);
    };

    let mut result = vec![root];
    let mut parent_ids = vec![root_run_id.to_string()];

    while !parent_ids.is_empty() {
        let children = run::Entity::find()
            .filter(run::Column::ParentRunId.is_in(&parent_ids))
            .all(db)
            .await?;
        parent_ids = children.iter().map(|c| c.id.clone()).collect();
        result.extend(children);
    }

    Ok(result)
}

pub async fn update_run_terminal_from_events(
    db: &DatabaseConnection,
    run_id: &str,
    events: &[(i64, String, String, i32)],
) -> Result<(), DbErr> {
    let Some((_, event_type, payload_str, _)) = events
        .iter()
        .rev()
        .find(|(_, event_type, _, _)| matches!(event_type.as_str(), "done" | "error"))
    else {
        return Ok(());
    };

    let payload: Value = serde_json::from_str(payload_str).unwrap_or(Value::Null);
    let model = match event_type.as_str() {
        "done" => run::ActiveModel {
            id: Set(run_id.to_string()),
            task_status: Set(Some("done".to_string())),
            error_message: Set(None),
            updated_at: Set(now()),
            ..Default::default()
        },
        "error" => run::ActiveModel {
            id: Set(run_id.to_string()),
            task_status: Set(Some("failed".to_string())),
            error_message: Set(Some(
                payload["message"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_string(),
            )),
            updated_at: Set(now()),
            ..Default::default()
        },
        _ => return Ok(()),
    };

    run::Entity::update(model).exec(db).await?;
    Ok(())
}
