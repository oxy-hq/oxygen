//! A process-wide ceiling on how many airhouse connections this Oxy process
//! may hold open at once, and the connect path that honours it.
//!
//! # Why this exists
//!
//! The pool caps each *logical identity* at N connections and caps how many
//! identities it tracks, in three separate budget classes. Those caps do not
//! compose into a safe total. Before this module, at the then-current defaults,
//! they multiplied out to
//!
//! ```text
//! (16 analytics + 16 app) x 3  +  8 preview x 3  =  120
//! ```
//!
//! connections from **one** Oxy process — and `oxy serve` runs several
//! replicas, each holding its own process-wide pool registry on top of that.
//! Airhouse's shared data plane admits a bounded number of connections per pod
//! across all tenants; that cap and the replica count are configured where the
//! data plane is deployed, not here, and the product above was never checked
//! against either. No per-identity cap could notice, because none of them can
//! see the others.
//!
//! The failure this prevents is connection *slots* running out while nothing is
//! actually overloaded: CPU and memory stay low, so a CPU-based autoscaler has
//! nothing to react to, while connects are refused at admission after waiting
//! out the data plane's queue.
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

/// The default process-wide ceiling, defined with the setting itself in
/// [`oxy_app_core::config::airhouse_pool`] so there is one source of truth for
/// it. Re-exported here because the tests below reason about it against the
/// pool's own caps.
///
/// Why it is a backstop and not a sized value: Oxy is not the data plane's only
/// client — airway ingest and the cameras fleet open their own connections —
/// and several `oxy serve` replicas and workers each hold a pool of their own.
/// So the relation that has to stay true is
///
/// ```text
/// ceiling x oxy_processes + other_consumers < per_pod_cap x dp_replicas
/// ```
///
/// Neither term on the right is configured in this repository: both belong to
/// the data plane's own deployment, which is why the default's job is to make a
/// runaway total impossible rather than to be the operating limit.
///
/// Size it per deployment from `internal-docs/airhouse-connection-scaling.md`,
/// which carries the server-side caps and a knob table, and raise
/// `OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS` together with that upstream capacity.
/// Note the interaction with width: at `OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY`
/// x the 6 analytics identities one workspace produces, a single busy workspace
/// can ask for more than this default allows, and would then meet
/// [`acquire`]'s refusal rather than queue upstream. That is the trade this
/// default makes deliberately, and the first thing to re-check when sizing.
use oxy_app_core::config::airhouse_pool::DEFAULT_MAX_LIVE_CONNS;

/// How long a connect waits for a permit before reporting the process as
/// locally saturated. Deliberately short: queueing here is cheaper than
/// queueing in airhouse's admission queue, but a request that waits many
/// seconds for a *local* slot is better off told so than left hanging.
const PERMIT_WAIT: Duration = Duration::from_secs(3);

/// How often [`acquire`] stops waiting to reclaim again. Short enough that a
/// burst finishing mid-wait is noticed well inside [`PERMIT_WAIT`], long enough
/// that a saturated process is not scanning the registry continuously.
const RECLAIM_TICK: Duration = Duration::from_millis(250);

fn permits() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(max_live_conns())))
}

/// The configured ceiling (`OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS`), from the
/// process's central configuration rather than a read at this call site.
///
/// Read once for the semaphore, when the first connection is opened: its size
/// cannot change afterwards, so setting the variable later has no effect.
pub fn max_live_conns() -> usize {
    oxy_app_core::config::BootConfig::current()
        .airhouse_pool
        .max_live_conns
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
///
/// Reclamation runs on a tick for the whole wait, not once up front. A pooled
/// connection's permit is **not** returned when the request borrowing it
/// finishes: the borrower's `Arc` drops and the connector goes back to being
/// idle in its slot, still holding the permit. So a burst that is in flight
/// when this is entered frees nothing on the first pass, and without a retick
/// the caller would wait out [`PERMIT_WAIT`] and be refused while every
/// connection sat idle. Ticking also covers tokio's FIFO-fair semaphore handing
/// a permit freed by a rebuild to an already-queued waiter.
async fn acquire(pool_key: &str) -> Result<OwnedSemaphorePermit, OxyError> {
    let sem = Arc::clone(permits());
    if let Ok(permit) = sem.clone().try_acquire_owned() {
        return Ok(permit);
    }
    tracing::info!(
        pool_key = %pool_key,
        ceiling = max_live_conns(),
        "airhouse pool: at the process connection ceiling, looking for a slot"
    );

    let deadline = tokio::time::Instant::now() + PERMIT_WAIT;
    loop {
        // Some of the budget may be held by connections nobody is using: a
        // pooled connection keeps its permit until its slot is reused or its
        // identity evicted, and a dead one keeps it just the same.
        let freed = super::reclaim_unused_permits(super::now_secs()).await;
        if freed > 0 {
            tracing::info!(
                pool_key = %pool_key,
                freed,
                "airhouse pool: reclaimed unused connection permits"
            );
        }
        if let Ok(permit) = sem.clone().try_acquire_owned() {
            return Ok(permit);
        }

        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        if left.is_zero() {
            return Err(OxyError::DBError(format!(
                "airhouse: this Oxy process already holds its maximum of {} airhouse \
                 connections and none came free within {}s. Raise \
                 OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS only if the data plane's own \
                 admission capacity has room for it.",
                max_live_conns(),
                PERMIT_WAIT.as_secs()
            )));
        }
        // Wait for a permit, but no longer than one tick: on a tick boundary go
        // back and reclaim again, because a connection that fell idle in the
        // meantime will not have returned its permit by itself.
        match tokio::time::timeout(RECLAIM_TICK.min(left), sem.clone().acquire_owned()).await {
            Ok(Ok(permit)) => return Ok(permit),
            // The semaphore is never closed, so this is unreachable in practice.
            Ok(Err(e)) => {
                return Err(OxyError::DBError(format!(
                    "airhouse connection budget unavailable: {e}"
                )));
            }
            Err(_) => continue,
        }
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
    fn the_per_identity_caps_alone_do_not_bound_the_total() {
        // Why a process-wide ceiling is needed at all: no per-identity or
        // per-class cap can see this product, so it is not a bound on anything.
        // It was 3 x (16 + 16 + 8) = 120 before this module, and raising
        // DEFAULT_MAX_IDENTITIES to cure the LRU churn widens it further —
        // which is only safe because the ceiling bounds it independently.
        //
        // How that product compares to what the data plane admits is not
        // checkable here: that capacity is configured with the data plane, not
        // in this repo. What is checkable, and what the ceiling relies on, is
        // that the caps leave the total free to grow by a wide margin.
        let uncapped = uncapped_at_defaults();
        assert!(
            uncapped >= DEFAULT_MAX_LIVE_CONNS * 4,
            "the caps leave the total at {uncapped}, close enough to the \
             ceiling of {DEFAULT_MAX_LIVE_CONNS} that the ceiling is barely \
             doing anything"
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

    /// The ceiling's job is to make one process's draw *predictable*, so a
    /// deployment can multiply it by its process count and compare the result
    /// against the data plane's capacity. Pin that it is small enough for that
    /// arithmetic to have anywhere to go: several Oxy processes at the ceiling
    /// still draw less than a single uncapped process could have.
    ///
    /// The comparison that actually matters — against the data plane's own
    /// admission cap times its replica count — cannot be made here, since
    /// neither figure is configured in this repo. It belongs to whoever sizes
    /// `OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS` for a deployment.
    #[test]
    fn several_processes_at_the_ceiling_draw_less_than_one_uncapped_process() {
        const PROCESSES: usize = 4;
        let capped = DEFAULT_MAX_LIVE_CONNS * PROCESSES;
        let uncapped = uncapped_at_defaults();
        assert!(
            capped < uncapped,
            "{PROCESSES} processes at the ceiling draw {capped}, more than the \
             {uncapped} one uncapped process would — the ceiling leaves a \
             deployment no room to scale out"
        );
    }

    #[test]
    fn the_local_wait_reports_saturation_rather_than_hanging() {
        // Waiting locally is only worthwhile while it stays much shorter than
        // the wait it replaces upstream. Past a few seconds a caller is better
        // served by an error naming the ceiling than by a request that hangs.
        assert!(PERMIT_WAIT < std::time::Duration::from_secs(10));
    }

    #[test]
    fn a_single_identity_cannot_reach_the_ceiling_on_its_own() {
        // N=3 per identity, so saturating 24 needs 8 distinct identities —
        // one tenant's traffic cannot lock the process out by itself.
        assert!(uncapped_ceiling(3, 1, 0, 0) < DEFAULT_MAX_LIVE_CONNS);
    }
}
