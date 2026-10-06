//! The execution stamp: whether an attempt has begun driving an analytics run,
//! and whether that attempt is still alive.
//!
//! A run driven from the request that started it has one driver for its whole
//! life. A run started from the task queue does not: a claim whose driver died
//! is handed to another, and a claim can even be handed on under a driver that
//! is still running (its queue heartbeat stalled and the reaper requeued it).
//! An analytics run keeps no checkpoint until it suspends, so a second attempt
//! would start from the top — a second copy of every `text_delta` on a stream
//! the client is reading, a second LLM bill, and every automation the agent
//! delegates to run twice.
//!
//! Two columns on `analytics_run_extensions` let a later attempt refuse:
//!
//! * `execution_started_at` — stamped by [`begin_execution`], one atomic write
//!   immediately before the pipeline starts. Only one attempt can ever win it.
//! * `execution_heartbeat_at` — written with the stamp and refreshed by
//!   [`beat_execution`] while the attempt lives. The stamp reads the same
//!   whether its attempt is dead or still running; the beat is what tells them
//!   apart.
//!
//! The same shape, for the same reason, as a custom-app procedure run's columns
//! on `customer_app_automation_runs`. The difference is where "still open" and
//! "cancel requested" live: on the run's own `agentic_runs` row, which the
//! statements below read in the same statement that writes.
//!
//! This module only records and reports. What an attempt does with the answer
//! — run, step aside, close the run as interrupted — is the caller's.

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, DatabaseBackend, DbErr, Statement};

use super::entity;

/// `agentic_runs` row `r` has not reached a terminal state.
const RUN_IS_OPEN: &str = "(r.task_status IS NULL \
     OR r.task_status NOT IN ('done', 'failed', 'cancelled', 'timed_out'))";

/// What a run's extension row records about its execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunExecution {
    /// When an attempt began executing the run. `None`: no attempt has.
    pub started_at: Option<DateTime<Utc>>,
    /// The executing attempt's newest proof of life.
    pub heartbeat_at: Option<DateTime<Utc>>,
}

impl RunExecution {
    /// When the executing attempt was last known alive: its newest beat, or
    /// the stamp itself for a row that has one and no beat. `None` when no
    /// attempt has begun the run.
    pub fn last_alive(&self) -> Option<DateTime<Utc>> {
        self.heartbeat_at.or(self.started_at)
    }
}

impl From<entity::Model> for RunExecution {
    fn from(row: entity::Model) -> Self {
        Self {
            started_at: row.execution_started_at.map(|at| at.with_timezone(&Utc)),
            heartbeat_at: row.execution_heartbeat_at.map(|at| at.with_timezone(&Utc)),
        }
    }
}

/// Take the run for this attempt, or learn that it is not this attempt's to
/// run. One atomic write: stamp `execution_started_at` on a run that is still
/// open, that nobody has asked to cancel, and that no attempt has begun.
///
/// `Ok(true)`: this attempt owns the run and may start its pipeline — and it
/// is the only attempt that ever will, because the stamp it just wrote is what
/// every later attempt reads. `Ok(false)`: not ours. The run is closed, a
/// cancel is pending, an earlier attempt already began it, or it has no
/// extension row; the caller re-reads to learn which.
///
/// "No attempt has begun" is checked on the row this statement locks, so two
/// attempts racing for the stamp cannot both win it. "Open" and "not
/// cancelled" are read from `agentic_runs` as of the statement: a cancel that
/// commits a moment later is not lost — the driver polls that flag — it is
/// only not *this* statement's to see.
///
/// The same write is the attempt's first heartbeat, so a begun run is never
/// without one.
pub async fn begin_execution<C: ConnectionTrait>(db: &C, run_id: &str) -> Result<bool, DbErr> {
    let now: DateTime<chrono::FixedOffset> = Utc::now().into();
    let sql = format!(
        "UPDATE analytics_run_extensions AS e \
         SET execution_started_at = $2, execution_heartbeat_at = $2 \
         WHERE e.run_id = $1 \
           AND e.execution_started_at IS NULL \
           AND EXISTS ( \
               SELECT 1 FROM agentic_runs r \
               WHERE r.id = e.run_id \
                 AND r.cancel_requested_at IS NULL \
                 AND {RUN_IS_OPEN})"
    );
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [run_id.into(), now.into()],
        ))
        .await?;
    Ok(res.rows_affected() > 0)
}

/// Re-stamp the heartbeat of a begun run that is still open. `Ok(false)` means
/// there is nothing left to prove: the run reached a terminal state, or was
/// never begun.
///
/// Guarded on the run being open and not on who holds its queue claim: the
/// claim is what a live attempt may have lost, and that is the case the beat
/// must survive. A suspended run is open too, so this statement does not
/// refuse one — but whether anything *calls* it for a suspended run is the
/// caller's choice. The queued ask's ticker does not: it stops when its
/// pipeline stops, a suspension included.
pub async fn beat_execution<C: ConnectionTrait>(db: &C, run_id: &str) -> Result<bool, DbErr> {
    let now: DateTime<chrono::FixedOffset> = Utc::now().into();
    let sql = format!(
        "UPDATE analytics_run_extensions AS e \
         SET execution_heartbeat_at = $2 \
         WHERE e.run_id = $1 \
           AND e.execution_started_at IS NOT NULL \
           AND EXISTS ( \
               SELECT 1 FROM agentic_runs r \
               WHERE r.id = e.run_id AND {RUN_IS_OPEN})"
    );
    let res = db
        .execute_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            sql,
            [run_id.into(), now.into()],
        ))
        .await?;
    Ok(res.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(secs: i64) -> Option<DateTime<Utc>> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0)
    }

    /// The beat is the newer fact; the stamp stands in only for a row that
    /// has one and no beat; and a run nobody began has neither.
    #[test]
    fn last_alive_is_the_newest_beat_else_the_stamp_else_nothing() {
        let beating = RunExecution {
            started_at: at(0),
            heartbeat_at: at(45),
        };
        assert_eq!(beating.last_alive(), at(45));

        let stamped_only = RunExecution {
            started_at: at(0),
            heartbeat_at: None,
        };
        assert_eq!(stamped_only.last_alive(), at(0));

        let unbegun = RunExecution {
            started_at: None,
            heartbeat_at: None,
        };
        assert_eq!(unbegun.last_alive(), None);
    }
}
