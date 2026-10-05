//! Closing a run nobody is driving.

use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DbErr, Statement};

use super::DRIVER_LEASE_TTL_SECS;

/// Fail a run **only if it has not ended and no driver holds a live lease on
/// it**. Returns whether the row was written.
///
/// One statement, so neither thing it must not do can be raced into between a
/// read and a write:
///
/// - rewrite a run that already reached `done` / `failed` / `cancelled` /
///   `timed_out` — [`super::update_run_failed`] is unconditional, and a cancel
///   that landed a moment after the run finished used to turn a `done` run
///   into a failed one;
/// - write a terminal state under a driver whose lease is live
///   (`driver_heartbeat_at` within [`DRIVER_LEASE_TTL_SECS`]). The driver
///   would keep running and write its own terminal state on top.
///
/// For a cancel endpoint's last resort: a run nothing is driving would
/// otherwise read `running` for ever. It stops nothing — stopping a live run
/// is [`super::request_cancel`]'s job, and this refuses to touch one.
///
/// The lease is the only liveness this reads. A caller that knows of another
/// (a `queued` / `claimed` queue entry) checks it first.
pub async fn fail_undriven_run(
    db: &DatabaseConnection,
    run_id: &str,
    error: &str,
) -> Result<bool, DbErr> {
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            "UPDATE agentic_runs \
             SET task_status = 'failed', error_message = $2, updated_at = now(), \
                 driver_id = NULL, driver_heartbeat_at = NULL \
             WHERE id = $1 \
               AND (task_status IS NULL \
                    OR task_status NOT IN ('done', 'failed', 'cancelled', 'timed_out')) \
               AND (driver_id IS NULL \
                    OR driver_heartbeat_at IS NULL \
                    OR driver_heartbeat_at < now() - make_interval(secs => $3))",
            [
                run_id.into(),
                error.into(),
                (DRIVER_LEASE_TTL_SECS as i32).into(),
            ],
        ))
        .await?;
    Ok(res.rows_affected() == 1)
}
