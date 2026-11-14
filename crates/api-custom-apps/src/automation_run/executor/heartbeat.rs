//! The executing attempt's proof of life, on the run's own row.
//!
//! `execution_started_at` says an attempt began the run; it cannot say whether
//! that attempt is dead or still running the steps. A claim can be handed on
//! under a live driver (its queue heartbeat stalled and the reaper requeued
//! it), and the next claimant reads the same stamp either way. So the attempt
//! that executes re-stamps `execution_heartbeat_at` here, and admission
//! ([`super::admission`]) reads it: fresh means someone is running this, stale
//! means nobody is.
//!
//! What it cannot do: tell a *stalled* driver from a dead one. An attempt
//! whose writes have not landed for [`STALE_AFTER_SECS`] reads as dead, here
//! as to the queue's reaper, and if it resumes after a later attempt closed
//! the run its result is discarded (`settle::close_running` only moves a
//! `running` row). It still never runs twice.

use std::time::Duration;

use chrono::{DateTime, Utc};
use entity::customer_app_procedure_runs as proc_run;
use entity::customer_app_procedure_runs::ActiveModel as ProcRunActiveModel;
use sea_orm::{
    ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter,
};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::Instrument;
use uuid::Uuid;

/// How often the executing attempt re-stamps the row: the queue claim's own
/// cadence (15 s), since both tick in one process over one pool and so stall
/// together.
pub(super) const INTERVAL: Duration = agentic_runtime::orchestrator::worker::HEARTBEAT_INTERVAL;

/// How many beats may be missed before the attempt reads as dead. The fourth
/// consecutive miss is what makes it stale.
const MISSED_BEATS_TOLERATED: u64 = 3;
// Never fewer than two: one slow write must not close a run under its driver.
const _: () = assert!(MISSED_BEATS_TOLERATED >= 2);

/// A heartbeat this old is a dead attempt's. `(MISSED_BEATS_TOLERATED + 1) ×
/// INTERVAL` = 60 s: the same silence after which the queue's reaper calls a
/// claim dead (`visibility_timeout_secs`), and no longer than a re-claim
/// already waits for a dead driver's lease (`DRIVER_LEASE_TTL_SECS` less one
/// lease beat), so the ordinary close of a dead attempt's run is not delayed.
pub(super) const STALE_AFTER_SECS: i64 = ((MISSED_BEATS_TOLERATED + 1) * INTERVAL.as_secs()) as i64;
// Two thirds of the lease's TTL is what is left of it one lease beat (TTL / 3)
// after a driver's last; a threshold past that would make every ordinary
// re-claim of a dead attempt's run step aside once before closing it.
const _: () = assert!(STALE_AFTER_SECS <= agentic_runtime::crud::DRIVER_LEASE_TTL_SECS * 2 / 3);

/// When the run's executing attempt was last known alive: its newest beat, or
/// the stamp itself for a run begun by a binary from before the heartbeat
/// column. `None` when no attempt has begun it.
pub(super) fn last_alive(row: &proc_run::Model) -> Option<DateTime<Utc>> {
    row.execution_heartbeat_at
        .or(row.execution_started_at)
        .map(|at| at.with_timezone(&Utc))
}

/// Has the attempt last alive at `last_alive` been silent too long to be
/// running? Both ends are application clocks, possibly on two pods; their skew
/// is small against a threshold of a minute.
pub(super) fn is_stale(last_alive: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(last_alive) >= chrono::Duration::seconds(STALE_AFTER_SECS)
}

/// Re-stamp the heartbeat, if the run is still `running`. `Ok(false)` means it
/// settled — by this attempt, a cancel, the poll or the sweep — and there is
/// nothing left to prove.
///
/// Guarded on the status and not on who holds the claim: the claim is what a
/// live attempt may have lost, and that is the case the beat must survive.
pub(super) async fn beat<C: ConnectionTrait>(db: &C, run_id: Uuid) -> Result<bool, DbErr> {
    let stamp = ProcRunActiveModel {
        execution_heartbeat_at: ActiveValue::Set(Some(Utc::now().into())),
        ..Default::default()
    };
    let res = proc_run::Entity::update_many()
        .set(stamp)
        .filter(proc_run::Column::Id.eq(run_id))
        .filter(proc_run::Column::Status.eq("running"))
        .exec(db)
        .await?;
    Ok(res.rows_affected > 0)
}

/// A running heartbeat. Dropping it stops the ticker; [`Beating::stop`] also
/// waits for it.
pub(super) struct Beating {
    stop: DropGuard,
    ticker: JoinHandle<()>,
}

impl Beating {
    /// Stop the ticker and wait for it to be gone.
    pub(super) async fn stop(self) {
        drop(self.stop);
        let _ = self.ticker.await;
    }

    /// The ticker's own task, for a test that waits for it to end by itself.
    #[cfg(test)]
    pub(super) fn into_ticker(self) -> (DropGuard, JoinHandle<()>) {
        (self.stop, self.ticker)
    }
}

/// Beat every `every` until the run settles, `cancel` trips (the task's own
/// token: a user cancel), or the returned handle is stopped or dropped.
///
/// The first beat is one interval out: `settle::begin_execution` wrote the
/// heartbeat with the stamp. A refresh that fails is logged and retried on the
/// next tick, never fatal to the run — a database blip must not end an
/// automation that is otherwise fine.
pub(super) fn spawn(
    db: DatabaseConnection,
    run_id: Uuid,
    cancel: CancellationToken,
    every: Duration,
) -> Beating {
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let ticker = tokio::spawn(
        async move {
            let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
            // After a stall, one beat and back on cadence — not a burst.
            ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                // Raced against the stop as well, so a write stuck on a
                // starved pool cannot hold up the outcome it is beside.
                let landed = tokio::select! {
                    _ = stopped.cancelled() => break,
                    _ = cancel.cancelled() => break,
                    landed = async { ticks.tick().await; beat(&db, run_id).await } => landed,
                };
                match landed {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => tracing::warn!(
                        %run_id,
                        error = %e,
                        "procedure run heartbeat failed; retrying next tick"
                    ),
                }
            }
        }
        .in_current_span(),
    );
    Beating {
        stop: stop.drop_guard(),
        ticker,
    }
}

#[cfg(test)]
mod tests;
