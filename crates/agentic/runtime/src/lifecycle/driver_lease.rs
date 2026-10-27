//! A direct drive's hold on the run's driver lease.
//!
//! A run is driven either by recovery (`recover_single_run`, which takes the
//! lease) or directly by the process that accepted the request — a chat run on
//! the `ide`, a custom-app ask on whichever `serve` replica took it, an
//! interactive automation or backfill. Direct drives used to take no lease, so
//! nothing in the database said they were alive: a peer's startup pass
//! (`get_resumable_root_runs`) selected them and a peer's boot cleanup stamped
//! or failed them. Holding the same lease recovery holds makes "a fresh lease"
//! the one liveness test for every driver.
//!
//! **From the row's first moment.** A chat run's row is inserted when its
//! start begins, and the start can take tens of seconds (a cold connector
//! build) before the drive exists. So the start stamps the lease right after
//! the insert (`agentic_pipeline::PipelineBuilder::start`) and the drive takes
//! it over under the same id ([`hold_direct_drive_lease_as`]). Without that, a
//! startup pass running during the start (the second one, after
//! `DRIVER_LEASE_TTL_SECS`) found a lease-free `running` row, retired it as
//! "has no checkpoint", and the start then drove it to `done` anyway.
//!
//! While held, a ticker:
//!
//! - heartbeats the lease every third of [`DRIVER_LEASE_TTL_SECS`], and stops
//!   once the heartbeat matches no row (the run went terminal — which clears
//!   the lease — or another driver took it over after a stall);
//! - polls the durable cancel flag every [`CANCEL_POLL`], and forwards it to
//!   this process's cancel channel. A cancel taken by another replica can no
//!   longer fail a leased run (`fail_undriven_run` refuses a live lease), so
//!   the flag is how it reaches this driver — the same flag recovery polls.
//!
//! On a graceful shutdown the host calls [`await_held_leases_released`] after
//! cancelling its runs: each cancelled drive releases its own lease as it
//! unwinds, so the next boot's first startup pass resumes it at once instead
//! of waiting out the TTL. A lease is never released for a drive that is
//! still running — that would let a peer re-drive a run this process is still
//! writing to.

use std::sync::Arc;
use std::time::Duration;

use agentic_core::hub_task::spawn_with_hub;
use dashmap::DashMap;
use sea_orm::DatabaseConnection;
use tokio::task::JoinHandle;

use super::crud::{
    DRIVER_LEASE_TTL_SECS, heartbeat_driver, is_cancel_requested, release_driver,
    try_acquire_driver,
};
use super::state::RuntimeState;

/// How often a held lease checks the durable cancel flag. Matches recovery's
/// cross-process cancel forwarder.
pub const CANCEL_POLL: Duration = Duration::from_secs(5);

/// Every direct-drive lease this process holds: run id → driver id.
static HELD: std::sync::LazyLock<DashMap<String, String>> = std::sync::LazyLock::new(DashMap::new);

/// Wait, up to `timeout`, for every direct drive in this process to release
/// its lease. For the graceful-shutdown hook, after the drives were
/// cancelled: each one releases its own lease once its coordinator returns.
/// Nothing is released on a drive's behalf — a drive still unwinding keeps
/// its lease, which lapses within the TTL. Returns how many are still held.
pub async fn await_held_leases_released(timeout: Duration) -> usize {
    let deadline = tokio::time::Instant::now() + timeout;
    while !HELD.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    HELD.len()
}

/// The lease a direct drive holds for as long as it runs. Call
/// [`DirectDriveLease::release`] when the drive ends; dropping it only stops
/// the ticker, leaving the lease to lapse within the TTL.
pub struct DirectDriveLease {
    db: DatabaseConnection,
    run_id: String,
    driver_id: Option<String>,
    ticker: Option<JoinHandle<()>>,
}

/// Take the driver lease on `run_id` for a drive starting in this process.
///
/// Never fails the drive: if the lease is held by another live driver, or the
/// acquire errors, the drive goes ahead exactly as it did before leases were
/// taken here, and the miss is logged.
pub async fn hold_direct_drive_lease(
    db: &DatabaseConnection,
    state: Arc<RuntimeState>,
    run_id: &str,
) -> DirectDriveLease {
    hold_direct_drive_lease_as(db, state, run_id, None).await
}

/// [`hold_direct_drive_lease`], continuing the lease `driver_id` already
/// stamped on the row — the start's, taken when it inserted the run. The
/// acquire accepts its own id, so this renews it and starts the ticker; a
/// fresh id would find the start's stamp live and drive without a lease.
/// `None` takes a fresh one.
pub async fn hold_direct_drive_lease_as(
    db: &DatabaseConnection,
    state: Arc<RuntimeState>,
    run_id: &str,
    driver_id: Option<String>,
) -> DirectDriveLease {
    let driver_id = driver_id.unwrap_or_else(|| format!("direct-{}", uuid::Uuid::new_v4()));
    let held = match try_acquire_driver(db, run_id, &driver_id).await {
        Ok(held) => held,
        Err(e) => {
            tracing::warn!(target: "runtime", run_id, error = %e, "direct drive: lease acquire failed");
            false
        }
    };
    if !held {
        tracing::info!(target: "runtime", run_id, "direct drive: another driver holds the lease");
        return DirectDriveLease {
            db: db.clone(),
            run_id: run_id.to_string(),
            driver_id: None,
            ticker: None,
        };
    }
    HELD.insert(run_id.to_string(), driver_id.clone());
    let ticker = spawn_with_hub(tick(
        db.clone(),
        state,
        run_id.to_string(),
        driver_id.clone(),
    ));
    DirectDriveLease {
        db: db.clone(),
        run_id: run_id.to_string(),
        driver_id: Some(driver_id),
        ticker: Some(ticker),
    }
}

impl DirectDriveLease {
    /// Whether this drive took the lease.
    pub fn is_held(&self) -> bool {
        self.driver_id.is_some()
    }

    /// Stop the ticker and give the lease back, if this drive still holds it.
    pub async fn release(mut self) {
        if let Some(t) = self.ticker.take() {
            t.abort();
        }
        let Some(driver_id) = self.driver_id.clone() else {
            return;
        };
        // Release in the database first, then leave `HELD`: the shutdown
        // hook's wait reads `HELD` empty as "every lease is back", so the
        // UPDATE must have run by then. (If this future is dropped mid-await,
        // `Drop` still clears the entry, since `driver_id` is not taken yet.)
        if let Err(e) = release_driver(&self.db, &self.run_id, &driver_id).await {
            tracing::warn!(
                target: "runtime",
                run_id = %self.run_id,
                error = %e,
                "direct drive: lease release failed; it lapses within the TTL"
            );
        }
        self.driver_id = None;
        HELD.remove_if(&self.run_id, |_, d| *d == driver_id);
    }
}

impl Drop for DirectDriveLease {
    fn drop(&mut self) {
        if let Some(t) = self.ticker.take() {
            t.abort();
        }
        if let Some(driver_id) = self.driver_id.take() {
            HELD.remove_if(&self.run_id, |_, d| *d == driver_id);
        }
    }
}

/// Heartbeat the lease and forward the durable cancel flag until the lease is
/// gone. Errors are logged and retried on the next tick: a missed beat costs
/// nothing until three in a row.
async fn tick(db: DatabaseConnection, state: Arc<RuntimeState>, run_id: String, driver_id: String) {
    let mut beat = tokio::time::interval(Duration::from_secs(
        (DRIVER_LEASE_TTL_SECS / 3).max(1) as u64
    ));
    let mut poll = tokio::time::interval(CANCEL_POLL);
    beat.tick().await;
    poll.tick().await;
    let mut cancel_forwarded = false;
    loop {
        tokio::select! {
            _ = beat.tick() => match heartbeat_driver(&db, &run_id, &driver_id).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(e) => tracing::warn!(
                    target: "runtime", run_id = %run_id, error = %e,
                    "direct drive: lease heartbeat failed; retrying next tick"
                ),
            },
            _ = poll.tick(), if !cancel_forwarded => {
                if is_cancel_requested(&db, &run_id).await.unwrap_or(false) {
                    tracing::info!(target: "runtime", run_id = %run_id, "direct drive: cancel requested elsewhere");
                    state.cancel(&run_id);
                    cancel_forwarded = true;
                }
            }
        }
    }
}
