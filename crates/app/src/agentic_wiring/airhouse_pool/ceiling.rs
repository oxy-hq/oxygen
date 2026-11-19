//! A process-wide ceiling on how many airhouse connections this Oxy process
//! may hold open at once, and the connect path that honours it.
//!
//! # Why this exists
//!
//! The pool caps each *logical identity* at N connections and caps how many
//! identities it tracks, in three separate budget classes. Those caps do not
//! compose into a safe total. At the defaults they multiply out to
//!
//! ```text
//! (16 analytics + 16 app) x 3  +  8 preview x 3  =  120
//! ```
//!
//! connections from **one** Oxy process — and `oxy serve` runs several
//! replicas. Airhouse's shared data plane admits **32 per pod across all
//! tenants** at 2 replicas: **64 in total**, with a 64-deep queue and a 30s
//! queue timeout. So the pool's own ceiling was nearly double the entire
//! capacity it was drawing on, and no single per-identity cap could notice.
//!
//! That is the shape of the 2026-11-13..15 incident in `oxy-prod`: 10,977
//! connections refused at admission, every one of them `reason: TimedOut`
//! after the full 30s wait, with CPU at 0% and memory at 33%. Nothing was
//! overloaded — the connection *slots* were gone, so the CPU-based HPA had
//! nothing to react to.
//!
//! A permit here is held for a connection's whole life, mirroring how airhouse
//! holds its own admission permit, and it is carried **by the connector**
//! ([`AirhouseConnector::holding_budget_permit`]) rather than by the pool slot.
//! That matters for one case the pool already documents: when an idle identity
//! is evicted while a request still holds its connector, the connection stays
//! open until that request finishes. A permit living in the slot would be
//! returned at eviction and the count would drift below the truth; living on
//! the connector it is released exactly when the socket closes.

use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use airhouse::AirhouseConnector;
use oxy_shared::errors::OxyError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::backpressure;

/// Default process-wide ceiling on open airhouse connections.
///
/// Airhouse's shared data plane offers 64 connection slots in total, and Oxy is
/// not its only client — airway ingest and the cameras fleet open their own.
/// At 24 per process, two `oxy serve` replicas can hold at most 48 of the 64,
/// leaving room for a third replica's burst and for the other consumers. Raise
/// it only together with `AIRHOUSE_MAX_CONNECTIONS` or the DP replica count:
/// the number that must stay true is
/// `ceiling x oxy_replicas + other_consumers < 32 x dp_replicas`.
const DEFAULT_MAX_LIVE_CONNS: usize = 24;

/// How long a connect waits for a permit before reporting the process as
/// locally saturated. Deliberately short: queueing here is cheaper than
/// queueing in airhouse's 30s admission queue, but a request that waits many
/// seconds for a *local* slot is better off told so than left hanging.
const PERMIT_WAIT: Duration = Duration::from_secs(3);

fn permits() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(max_live_conns())))
}

/// The configured ceiling (`OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS`).
///
/// Read once, when the first connection is opened: the semaphore's size cannot
/// change afterwards, so setting the variable later has no effect.
pub fn max_live_conns() -> usize {
    super::env_usize("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", DEFAULT_MAX_LIVE_CONNS)
}

/// Open an airhouse connection against the process-wide budget, retrying while
/// airhouse answers backpressure.
///
/// `open` is called once per attempt and must only *connect* — it is retried,
/// so it must carry no side effect to repeat. The credential is minted by the
/// caller before this runs and is not re-minted between attempts: the whole
/// retry budget is under two seconds against a 900s TTL.
///
/// The returned connector holds a permit for as long as it stays open.
pub async fn connect_within_budget<F, Fut>(
    pool_key: &str,
    open: F,
) -> Result<AirhouseConnector, OxyError>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<AirhouseConnector, OxyError>>,
{
    let permit = acquire(pool_key).await?;
    let mut attempt = 0usize;
    loop {
        match open().await {
            Ok(conn) => return Ok(conn.holding_budget_permit(permit)),
            Err(e) => {
                if !backpressure::is_backpressure(&e.to_string()) {
                    // Not "not now" — an auth failure, a missing database, a
                    // misconfiguration. Retrying only delays the same answer.
                    return Err(e);
                }
                match backpressure::retry_delay(attempt) {
                    Some(delay) => {
                        tracing::info!(
                            pool_key = %pool_key,
                            attempt = attempt + 1,
                            of = backpressure::max_attempts(),
                            delay_ms = delay.as_millis(),
                            "airhouse pool: data plane at capacity, retrying connect"
                        );
                        tokio::time::sleep(delay).await;
                        attempt += 1;
                    }
                    // Out of attempts: the data plane is not transiently busy
                    // but genuinely full. The caller gets airhouse's own error.
                    None => return Err(e),
                }
            }
        }
    }
}

/// Take a slot in the process-wide budget, or report local saturation.
async fn acquire(pool_key: &str) -> Result<OwnedSemaphorePermit, OxyError> {
    let sem = Arc::clone(permits());
    if let Ok(permit) = sem.clone().try_acquire_owned() {
        return Ok(permit);
    }
    tracing::info!(
        pool_key = %pool_key,
        ceiling = max_live_conns(),
        "airhouse pool: at the process connection ceiling, waiting for a slot"
    );
    match tokio::time::timeout(PERMIT_WAIT, sem.acquire_owned()).await {
        Ok(Ok(permit)) => Ok(permit),
        // The semaphore is never closed, so this arm is unreachable in practice.
        Ok(Err(e)) => Err(OxyError::DBError(format!(
            "airhouse connection budget unavailable: {e}"
        ))),
        Err(_) => Err(OxyError::DBError(format!(
            "airhouse: this Oxy process already holds its maximum of {} airhouse \
             connections and none came free within {}s. Raise \
             OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS only if airhouse's own capacity \
             (AIRHOUSE_MAX_CONNECTIONS x data-plane replicas) has room for it.",
            max_live_conns(),
            PERMIT_WAIT.as_secs()
        ))),
    }
}

/// What the per-identity and per-class caps multiply out to with no process
/// ceiling: the number this module exists to replace. Pure, so the arithmetic
/// that justifies the ceiling is checked rather than asserted in a comment.
///
/// Test-only — nothing in the running pool needs this figure, only the tests
/// that keep the ceiling honest against the caps.
#[cfg(test)]
pub(super) fn uncapped_ceiling(
    conns_per_identity: usize,
    max_analytics: usize,
    max_app: usize,
    max_preview: usize,
) -> usize {
    conns_per_identity * (max_analytics + max_app + max_preview)
}

#[cfg(test)]
mod tests {
    use super::super::{
        DEFAULT_CONNS_PER_IDENTITY, DEFAULT_MAX_APP_IDENTITIES, DEFAULT_MAX_IDENTITIES,
        DEFAULT_MAX_PREVIEW_IDENTITIES,
    };
    use super::{DEFAULT_MAX_LIVE_CONNS, PERMIT_WAIT, uncapped_ceiling};

    /// `AIRHOUSE_MAX_CONNECTIONS` x data-plane replicas, as deployed.
    ///
    /// Not read from this repo — Airhouse's own config is in its repo. If that
    /// cap or the DP replica count changes, this constant and the ceiling's
    /// default have to move together.
    const AIRHOUSE_TOTAL_SLOTS: usize = 32 * 2;

    /// Reads the live defaults rather than copies of them, so the pool's own
    /// caps cannot drift away from the arithmetic that justifies the ceiling.
    fn uncapped_at_defaults() -> usize {
        uncapped_ceiling(
            DEFAULT_CONNS_PER_IDENTITY,
            DEFAULT_MAX_IDENTITIES,
            DEFAULT_MAX_APP_IDENTITIES,
            DEFAULT_MAX_PREVIEW_IDENTITIES,
        )
    }

    #[test]
    fn the_per_identity_caps_alone_overshoot_airhouse_entirely() {
        // Why a process-wide ceiling is needed at all: no per-identity or
        // per-class cap can see this total. It was already ~2x Airhouse's whole
        // capacity at the pre-incident caps (3 x (16 + 16 + 8) = 120), and
        // raising DEFAULT_MAX_IDENTITIES to cure the LRU churn widens it
        // further — which is only safe because the ceiling bounds it.
        let uncapped = uncapped_at_defaults();
        assert!(
            uncapped > AIRHOUSE_TOTAL_SLOTS,
            "one Oxy process could hold {uncapped} of airhouse's \
             {AIRHOUSE_TOTAL_SLOTS} slots"
        );
    }

    #[test]
    fn the_ceiling_is_what_actually_bounds_the_process() {
        // The invariant the whole module exists for: whatever the caps multiply
        // out to, the number of live connections cannot pass the ceiling.
        assert!(
            DEFAULT_MAX_LIVE_CONNS < uncapped_at_defaults(),
            "the ceiling must be the binding constraint, not the caps"
        );
    }

    /// `airhouse_pool_key` yields 6 analytics identities per workspace (4
    /// roles + role-less + system). Past the cap, every new-identity insert
    /// evicts down to `max - 1` and the evicted identity's next request pays a
    /// fresh mint + connect + schema fetch — connect churn that scales with
    /// tenant count, not query volume. 16 was exceeded by three workspaces.
    #[test]
    fn the_analytics_budget_holds_several_workspaces_without_lru_churn() {
        const IDENTITIES_PER_WORKSPACE: usize = 6;
        let workspaces = DEFAULT_MAX_IDENTITIES / IDENTITIES_PER_WORKSPACE;
        assert!(
            workspaces >= 8,
            "the analytics budget holds only {workspaces} workspaces before it \
             starts thrashing"
        );
    }

    #[test]
    fn the_default_ceiling_leaves_airhouse_room_for_other_replicas() {
        assert!(
            DEFAULT_MAX_LIVE_CONNS < AIRHOUSE_TOTAL_SLOTS,
            "one process must not be able to take every slot"
        );
        // Two oxy-serve replicas must still leave slots for airway ingest and
        // the cameras fleet, which open their own connections.
        assert!(
            DEFAULT_MAX_LIVE_CONNS * 2 < AIRHOUSE_TOTAL_SLOTS,
            "{DEFAULT_MAX_LIVE_CONNS} x 2 replicas must stay under \
             {AIRHOUSE_TOTAL_SLOTS}"
        );
    }

    #[test]
    fn waiting_locally_is_far_cheaper_than_airhouses_admission_queue() {
        // If the local wait approached airhouse's 30s queue timeout we would
        // just be reproducing the incident's latency in a different process.
        assert!(PERMIT_WAIT < std::time::Duration::from_secs(10));
    }

    #[test]
    fn a_single_identity_cannot_reach_the_ceiling_on_its_own() {
        // N=3 per identity, so saturating 24 needs 8 distinct identities —
        // one tenant's traffic cannot lock the process out by itself.
        assert!(uncapped_ceiling(3, 1, 0, 0) < DEFAULT_MAX_LIVE_CONNS);
    }
}
