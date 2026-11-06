//! The boot-time reconciliation of non-terminal `agentic_runs` rows.
//!
//! Called once per process start by the processes that resume runs — `oxy
//! serve` in role `ide` / `all` (`oxy-app`'s `new_agentic_state`). A `serve`
//! replica (`--no-workers`) and `oxy worker` never call it: they resume
//! nothing, so every write here would be about runs other processes own.
//!
//! **Liveness.** Even from the right process, a boot says nothing about runs a
//! live *peer* is driving — a worker draining the queue, a serve replica
//! direct-driving a custom-app ask. A run is held, and this module leaves it
//! alone, when either is true (both read on the database clock):
//!
//! - **a fresh driver lease** — `driver_id` set and `driver_heartbeat_at`
//!   within [`DRIVER_LEASE_TTL_SECS`]. Recovery drives and direct drives both
//!   take it (`lifecycle::driver_lease`) and heartbeat it every third of the
//!   TTL;
//! - **a live queue entry anywhere in its task tree** — `queued` or `claimed`.
//!   Whether a claim is dead is the reaper's rule, not this module's
//!   (`reap_stale_tasks`); a second copy here could only disagree with it.
//!
//! **No overwrites.** Every arm is one set-based `UPDATE` whose `WHERE` names
//! the non-terminal statuses it may rewrite, so a peer's `done` committed
//! after this process started reading is re-checked by Postgres and skipped —
//! never replaced by `needs_resume` or `failed`.
//!
//! Arms, applied in this order (their predicates are disjoint):
//!
//! | run | written |
//! | --- | --- |
//! | `waiting_on_child` / `waiting_on_children`, not held | `failed` (dormant: a coordinator waiting on children persists `delegating`) |
//! | root with no events, not held | `failed`, "run never started" |
//! | anything else in `running` / `delegating` / `needs_resume` / `shutdown`, not held | `needs_resume` and a note |
//! | `awaiting_input` | nothing — the user resumes it |
//! | child of a `done` / `failed` parent, still non-terminal | `failed` |

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};

use super::recovery::queue_rows_of_subtree;
use crate::lifecycle::crud::DRIVER_LEASE_TTL_SECS;

/// SQL predicate over run `r`: nothing live holds it.
///
/// `r` may be a child run here (the `waiting_on_child` and `needs_resume`
/// arms take children), and a child is held by its own subtree, not its whole
/// tree — so this keeps the subtree match rather than `root_task_id`.
fn not_held() -> String {
    let queue_rows = queue_rows_of_subtree("q");
    format!(
        "NOT (r.driver_id IS NOT NULL \
              AND r.driver_heartbeat_at IS NOT NULL \
              AND r.driver_heartbeat_at >= now() - make_interval(secs => {DRIVER_LEASE_TTL_SECS})) \
         AND NOT EXISTS ( \
             SELECT 1 FROM agentic_task_queue q \
             WHERE {queue_rows} AND q.queue_status IN ('queued', 'claimed'))"
    )
}

const HAS_EVENTS: &str = "EXISTS (SELECT 1 FROM agentic_run_events e WHERE e.run_id = r.id)";

/// One arm: `UPDATE ... SET task_status = <to>, error_message = <note>` over
/// the rows matching `predicate`. Returns rows written.
async fn apply_arm(
    db: &DatabaseConnection,
    to: &str,
    note: &str,
    predicate: &str,
) -> Result<u64, DbErr> {
    let sql = format!(
        "UPDATE agentic_runs r \
         SET task_status = $1, error_message = $2, updated_at = now() \
         WHERE {predicate}"
    );
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [to.into(), note.into()],
        ))
        .await?;
    Ok(res.rows_affected())
}

/// Reconcile the non-terminal runs nothing live holds. Returns rows written.
/// See the module docs for which process may call it and what it spares.
pub async fn cleanup_stale_runs(db: &DatabaseConnection) -> Result<u64, DbErr> {
    let free = not_held();
    let mut reconciled = 0;

    reconciled += apply_arm(
        db,
        "failed",
        "server restarted: delegation was interrupted",
        &format!("r.task_status IN ('waiting_on_child', 'waiting_on_children') AND {free}"),
    )
    .await?;

    reconciled += apply_arm(
        db,
        "failed",
        "server restarted: run never started",
        &format!(
            "r.parent_run_id IS NULL \
             AND r.task_status IN ('running', 'delegating', 'needs_resume', 'shutdown') \
             AND NOT {HAS_EVENTS} AND {free}"
        ),
    )
    .await?;

    reconciled += apply_arm(
        db,
        "needs_resume",
        "server restarted: run will be resumed automatically",
        &format!(
            "r.task_status IN ('running', 'delegating', 'needs_resume', 'shutdown') \
             AND (r.parent_run_id IS NOT NULL OR {HAS_EVENTS}) AND {free}"
        ),
    )
    .await?;

    reconciled += apply_arm(
        db,
        "failed",
        "parent task completed; orphaned child cleaned up",
        "r.parent_run_id IS NOT NULL \
         AND r.task_status IN ('needs_resume', 'running', 'shutdown', 'waiting_on_children', \
                               'waiting_on_child', 'awaiting_input', 'delegating') \
         AND EXISTS (SELECT 1 FROM agentic_runs p \
                     WHERE p.id = r.parent_run_id AND p.task_status IN ('done', 'failed'))",
    )
    .await?;

    Ok(reconciled)
}
