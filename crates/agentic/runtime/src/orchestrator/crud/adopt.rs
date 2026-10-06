//! Taking one named `queued` task as a claim, without running it.

use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};

/// Claim the `queued` task `task_id` for `worker_id`. `Ok(false)` when the row
/// is not `queued` (absent, already claimed, or terminal) — nothing is written.
///
/// For a task whose work is **already under way and parked**: a run that
/// reached a suspension keeps its claim so the same `task_id` can resume in
/// place, and when the driver holding it dies the row comes back `queued`
/// carrying the spec it was first claimed with. A worker that claimed it the
/// ordinary way ([`super::queue::claim_task_under_root`]) would be handed that
/// spec and start the run again from the top. The driver recovering the run
/// takes the row back here instead, so it reads as what it is — held by a
/// driver, waiting to resume.
///
/// Two differences from an ordinary claim, both deliberate:
///
/// - **`available_at` is not consulted.** The row is not being handed out to
///   run; a deferral's delay has nothing to say about who holds it.
/// - **`claim_count` is not charged.** That budget bounds how often a task may
///   be *run* and fail (`max_claims`, after which the reaper dead-letters it).
///   Nothing runs here, and a run parked on a person across three restarts has
///   not failed at anything.
///
/// The caller owes the row a heartbeat from this moment, like any other claim
/// (`DurableTransport::adopt_queued_claim` is the one that pairs them).
pub async fn adopt_queued_task<C: ConnectionTrait>(
    db: &C,
    task_id: &str,
    worker_id: &str,
) -> Result<bool, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_task_queue \
             SET queue_status = 'claimed', \
                 worker_id = $2, \
                 claimed_at = now(), \
                 last_heartbeat = now(), \
                 updated_at = now() \
             WHERE task_id = $1 AND queue_status = 'queued'",
            [task_id.into(), worker_id.into()],
        ))
        .await?;
    Ok(res.rows_affected() > 0)
}
