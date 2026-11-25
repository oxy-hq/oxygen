//! The end of a pass, once its lease is released (step 5 of a pass, in the
//! module docs of `relay`): the mark's backoff, then its drain or the next
//! pass.
//!
//! - **Backoff.** A pass that failed after taking the lease (its run failed or
//!   stopped, or any writer failed) backs the mark off ([`marks::failed`]):
//!   the sweep leaves it until [`BACKOFF_BASE`] doubled per earlier failure,
//!   at most [`BACKOFF_CAP`], unless a nudge comes after the failure. A pass
//!   with no failed writer, drained or not, ends the backoff.
//! - **Drain.** The mark is drained as of the pass's start only when no writer
//!   failed and none stopped with rows left (a held row is not left).
//! - **Next pass.** Rows left with no writer failed queue the next pass now,
//!   rather than at the sweep; a failure waits out its backoff instead.
//!
//! A failure to record any of these is logged, and leaves the mark due.

use chrono::{DateTime, Utc};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::{BACKOFF_BASE, BACKOFF_CAP, REDRAIN_MARGIN, RelayStats, enqueue_relay, marks};

/// What a pass that ran made of it.
pub(super) struct Ran {
    pub stats: RelayStats,
    pub started: DateTime<Utc>,
    /// Each failed writer, as `<writer>: <error>`.
    pub failed: Vec<String>,
    /// Whether rows may still wait outside a hold.
    pub left: bool,
}

impl Ran {
    /// A pass that found nothing to relay, begun at `started`.
    pub fn nothing(started: DateTime<Utc>) -> Self {
        Self {
            stats: RelayStats::default(),
            started,
            failed: Vec::new(),
            left: false,
        }
    }
}

/// Settle the mark after `ran`, the outcome of the pass under the lease:
/// `Err` when it stopped, `None` when it found a bump re-stamping.
pub(super) async fn settle(
    db: &DatabaseConnection,
    org_id: Uuid,
    environment: &str,
    ran: &Result<Option<Ran>, String>,
) {
    let failed = match ran {
        Err(_) => true,
        Ok(Some(ran)) => !ran.failed.is_empty(),
        Ok(None) => false,
    };
    let backoff = if failed {
        marks::failed(db, org_id, environment, BACKOFF_BASE, BACKOFF_CAP).await
    } else {
        marks::succeeded(db, org_id, environment).await
    };
    if let Err(e) = backoff {
        tracing::warn!(%org_id, environment, failed, error = %e, "oltp_changes: could not record the relay pass's backoff");
    }
    if let Ok(Some(ran)) = ran
        && ran.failed.is_empty()
    {
        drain_or_requeue(db, org_id, environment, ran).await;
    }
}

/// With no writer failed: the mark drained when nothing is left, the next
/// pass queued when rows are.
async fn drain_or_requeue(db: &DatabaseConnection, org_id: Uuid, environment: &str, ran: &Ran) {
    if !ran.left {
        let drained = marks::drained(db, org_id, environment, ran.started, REDRAIN_MARGIN).await;
        if let Err(e) = drained {
            tracing::warn!(%org_id, environment, error = %e, "oltp_changes: could not mark the change stream drained");
        }
    } else if let Err(e) = enqueue_relay(db, org_id, environment).await {
        tracing::warn!(%org_id, environment, error = %e, "oltp_changes: could not queue the next relay pass");
    }
}
