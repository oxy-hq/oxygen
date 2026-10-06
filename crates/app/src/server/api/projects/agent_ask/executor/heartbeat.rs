//! The executing attempt's proof of life, on the ask's own row.
//!
//! `execution_started_at` says an attempt began the ask; it cannot say whether
//! that attempt is dead or still running the pipeline. A claim can be handed
//! on under a live driver (its queue heartbeat stalled and the reaper requeued
//! it), and the next claimant reads the same stamp either way. So the attempt
//! that executes re-stamps `execution_heartbeat_at` here, and admission
//! ([`super::admission`]) reads it: fresh means someone is running this, stale
//! means nobody is.
//!
//! It beats from the stamp until this attempt's pipeline stops. That is a
//! terminal outcome, or a suspension: a pipeline that suspends reports
//! `Suspended` and its task ends, dropping the channels the executor's bridge
//! is waiting on (`StartedPipeline::into_executing_task`'s join handle). If a
//! producer ever keeps a sender open past that point, the ticker simply runs
//! on until the run ends, when the beat itself answers `false`.
//!
//! Stopping at a suspension is a saving, not something anything relies on:
//! admission checks for a suspension *before* it reads the heartbeat, so the
//! heartbeat of a suspended run is never consulted, beating or not. Past a
//! suspension the run is continued from its checkpoint by recovery.
//!
//! What it cannot do: tell a *stalled* driver from a dead one. An attempt
//! whose writes have not landed for [`STALE_AFTER_SECS`] reads as dead, here as
//! to the queue's reaper. It still never runs twice.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::{CancellationToken, DropGuard};
use tracing::Instrument;

/// How often the executing attempt re-stamps the row: the queue claim's own
/// cadence (15 s), since both tick in one process over one pool and so stall
/// together.
pub(super) const INTERVAL: Duration = agentic_runtime::orchestrator::worker::HEARTBEAT_INTERVAL;

/// How many beats may be missed before the attempt reads as dead. The fourth
/// consecutive miss is what makes it stale.
const MISSED_BEATS_TOLERATED: u64 = 3;
// Never fewer than two: one slow write must not close an ask under its driver.
const _: () = assert!(MISSED_BEATS_TOLERATED >= 2);

/// A heartbeat this old is a dead attempt's. `(MISSED_BEATS_TOLERATED + 1) ×
/// INTERVAL` = 60 s: the same silence after which the queue's reaper calls a
/// claim dead (`visibility_timeout_secs`), and the number a procedure run uses
/// for the same question.
pub(super) const STALE_AFTER_SECS: i64 = ((MISSED_BEATS_TOLERATED + 1) * INTERVAL.as_secs()) as i64;
// Two thirds of the lease's TTL is what is left of it one lease beat (TTL / 3)
// after a driver's last; a threshold past that would make every ordinary
// re-claim of a dead attempt's ask step aside once before closing it.
const _: () = assert!(STALE_AFTER_SECS <= agentic_runtime::crud::DRIVER_LEASE_TTL_SECS * 2 / 3);

/// Has the attempt last alive at `last_alive` been silent too long to be
/// running? Both ends are application clocks, possibly on two pods; their skew
/// is small against a threshold of a minute.
pub(super) fn is_stale(last_alive: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    now.signed_duration_since(last_alive) >= chrono::Duration::seconds(STALE_AFTER_SECS)
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
}

/// Beat every `every` until the run ends or the returned handle is stopped or
/// dropped.
///
/// The first beat is one interval out: `begin_execution` wrote the heartbeat
/// with the stamp. A refresh that fails is logged and retried on the next
/// tick, never fatal to the ask — a database blip must not end a run that is
/// otherwise fine.
pub(super) fn spawn(db: DatabaseConnection, run_id: String, every: Duration) -> Beating {
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let ticker = tokio::spawn(
        async move {
            let mut ticks = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
            // After a stall, one beat and back on cadence — not a burst.
            ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                // Raced against the stop as well, so a write stuck on a
                // starved pool cannot outlive the pipeline it is beside.
                let landed = tokio::select! {
                    _ = stopped.cancelled() => break,
                    landed = async {
                        ticks.tick().await;
                        agentic_pipeline::run_execution::beat_execution(&db, &run_id).await
                    } => landed,
                };
                match landed {
                    Ok(true) => {}
                    // The run ended: nothing left to prove.
                    Ok(false) => break,
                    Err(e) => tracing::warn!(
                        %run_id,
                        error = %e,
                        "agent ask heartbeat failed; retrying next tick"
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
mod tests {
    use super::*;

    fn at(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("in range")
    }

    /// Three missed beats are tolerated; the fourth is what reads as dead. One
    /// second short of the threshold is still a live attempt.
    #[test]
    fn an_attempt_is_stale_only_after_four_missed_beats() {
        assert_eq!(STALE_AFTER_SECS, 60);
        assert!(!is_stale(at(0), at(0)));
        assert!(!is_stale(at(0), at(45)), "three missed beats");
        assert!(!is_stale(at(0), at(59)));
        assert!(is_stale(at(0), at(60)), "the fourth");
        assert!(is_stale(at(0), at(3600)));
    }

    /// A beat stamped by a pod whose clock runs ahead reads as fresh, never as
    /// a negative age that some comparison might treat as stale.
    #[test]
    fn a_beat_from_a_clock_running_ahead_is_fresh() {
        assert!(!is_stale(at(30), at(0)));
    }
}
