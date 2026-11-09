//! Ending the run of a task the queue dead-lettered.
//!
//! A queue row reaches `dead` two ways: the reaper finds its claim abandoned
//! with `max_claims` spent ([`super::queue::reap_stale_tasks`]), or a deferral
//! waits past its domain's ceiling ([`super::queue::defer_task`]). Either way
//! nothing claims the row again, so unless something ends the run it reads
//! `running` for ever: `find_pending_global_runs` wants a `queued` row and no
//! alert keys on a run that looks in flight.
//!
//! Who ends it follows the liveness rule (`internal-docs/worker-fleet.md`,
//! "How a process decides a run is alive"). A run with a fresh driver lease,
//! or with another entry of its tree still `queued` / `claimed`, is held: its
//! driver hears the dead-letter as a `Failed` outcome (the deferral path
//! forwards one; a reaper dead-letter under a live driver reaches it as the
//! lost claim's outcome, rewritten by `DurableTransport`) and ends the run
//! through the coordinator, which applies the parent's retry policy. Only a
//! run nothing holds is failed here, in one statement that re-checks both
//! conditions and never rewrites a terminal status.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};

use crate::lifecycle::crud::DRIVER_LEASE_TTL_SECS;

/// The run error for a task dead-lettered by a deferral ceiling.
pub fn deferral_dead_letter_message(reason: &str) -> String {
    format!("dead-lettered after waiting past its queue ceiling: {reason}")
}

/// The run error for a task the reaper dead-lettered: every claim's worker
/// stopped heartbeating before the task finished.
pub fn reaper_dead_letter_message(claims: i32) -> String {
    format!(
        "dead-lettered after {claims} claims: each worker stopped heartbeating before \
         the task finished"
    )
}

/// What [`fail_dead_lettered_run`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeadLetterRun {
    /// The root run (and its non-terminal descendants) is now `failed`.
    Failed { run_id: String },
    /// Left alone: already terminal, or held by a live driver or a live
    /// queue entry, whose driver ends it.
    Held { run_id: String },
    /// No run row owns the task.
    NoRun,
}

/// Fail the root run owning dead-lettered `task_id`, **unless something still
/// holds it**. See the module docs for the rule.
pub async fn fail_dead_lettered_run(
    db: &DatabaseConnection,
    task_id: &str,
    error: &str,
) -> Result<DeadLetterRun, DbErr> {
    let Some(run_id) = super::recovery::owning_run_id(db, task_id).await? else {
        return Ok(DeadLetterRun::NoRun);
    };
    let rows = db
        .query_all_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            FAIL_UNHELD_TREE,
            [
                run_id.clone().into(),
                error.into(),
                (DRIVER_LEASE_TTL_SECS as i32).into(),
            ],
        ))
        .await?;
    let root_failed = rows
        .iter()
        .any(|r| r.try_get::<String>("", "id").is_ok_and(|id| id == run_id));
    if !root_failed {
        return Ok(DeadLetterRun::Held { run_id });
    }
    // Row-only terminal transition: wake the run's streams to it.
    // Best-effort: the row is committed, and the failure observer below must
    // run whether or not a second acquire succeeds.
    crate::lifecycle::crud::notify_run_events_best_effort(db, &run_id).await;
    crate::lifecycle::failure_hook::root_run_failed(&run_id).await;
    Ok(DeadLetterRun::Failed { run_id })
}

/// `$1` root run id, `$2` error, `$3` lease TTL. The `root` CTE is the whole
/// guard — not terminal, no fresh lease, no live entry in the tree — and is
/// locked so a driver taking the lease meanwhile waits for this write and
/// then sees a terminal run. The queue side keys on `root_task_id`; the run
/// side is `retire_run`'s `LIKE` (`agentic_runs` has no root column).
const FAIL_UNHELD_TREE: &str = "\
    WITH root AS ( \
        SELECT id FROM agentic_runs \
         WHERE id = $1 \
           AND (task_status IS NULL \
                OR task_status NOT IN ('done', 'failed', 'cancelled', 'timed_out')) \
           AND (driver_id IS NULL OR driver_heartbeat_at IS NULL \
                OR driver_heartbeat_at < now() - make_interval(secs => $3)) \
           AND NOT EXISTS ( \
               SELECT 1 FROM agentic_task_queue q \
                WHERE q.root_task_id = $1 \
                  AND q.queue_status IN ('queued', 'claimed')) \
         FOR UPDATE) \
    UPDATE agentic_runs r \
       SET task_status = 'failed', error_message = $2, \
           driver_id = NULL, driver_heartbeat_at = NULL, updated_at = now() \
      FROM root \
     WHERE (r.id = root.id OR r.id LIKE root.id || '.%') \
       AND (r.task_status IS NULL \
            OR r.task_status NOT IN ('done', 'failed', 'cancelled', 'timed_out')) \
    RETURNING r.id";
