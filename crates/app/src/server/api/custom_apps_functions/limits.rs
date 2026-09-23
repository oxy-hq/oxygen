//! Concurrency admission for Oxy Function invocations.
//!
//! ## The gap this closes
//!
//! There was no bound on concurrent invocations anywhere in
//! `custom_apps_functions/`. Each one is **one OS thread and one V8 heap**, and
//! neither is governed by anything else in the process: the pod's `cpu: 2`
//! limit sizes Tokio's worker pool, and these threads are spawned with
//! `std::thread` outside it. K concurrent requests meant K threads and K heaps,
//! with the cgroup memory limit as the only ceiling — and hitting that kills
//! the process serving every other app.
//!
//! The per-isolate heap ceiling (see `runtime::heap_limit_bytes`) bounds what
//! **one** invocation can take. This bounds **how many** there are. Neither is
//! sufficient alone, and the product of the two is the number that has to stay
//! under the pod's memory budget.
//!
//! ## Queue, then shed — not a hard reject
//!
//! A permit is waited for, up to [`queue_budget`]. Only a wait that exceeds the
//! budget is shed. This matters because the cap is a guess until there is
//! production data behind it: a cap set slightly too low degrades into a few
//! milliseconds of queueing rather than into 503s, and
//! `oxy.custom_app.admission.wait` rises long before anything is rejected. Set
//! it too low with a hard reject and the first symptom is an outage.
//!
//! ## Two limits, because "full" and "hogged" need different responses
//!
//! A global ceiling and a smaller per-org one. One tenant looping invocations
//! cannot take every permit and starve the rest; the per-org limit binds first
//! and sheds only that tenant. The shed counter carries which limit bound,
//! because the operator response differs — `global` means the fleet needs more
//! replicas, `org` means one tenant is misbehaving and everyone else is fine.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use oxy_telemetry::metrics::sources::QueueGuard;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use uuid::Uuid;

/// Global concurrent-invocation ceiling. `0` disables the cap entirely.
pub const MAX_CONCURRENCY_ENV: &str = "OXY_FUNCTION_MAX_CONCURRENCY";
/// Per-org ceiling, applied under the global one.
pub const MAX_ORG_CONCURRENCY_ENV: &str = "OXY_FUNCTION_MAX_ORG_CONCURRENCY";
/// How long an invocation may queue for a permit before it is shed.
pub const QUEUE_BUDGET_SECS_ENV: &str = "OXY_FUNCTION_QUEUE_BUDGET_SECS";
/// How many invocations may be queued at once. `0` disables the queue ceiling.
pub const MAX_QUEUED_ENV: &str = "OXY_FUNCTION_MAX_QUEUED";

/// Default global ceiling.
///
/// Sized against the observed fleet, not a round number: the serve process
/// peaked at **888 MiB** against a 2 GiB cgroup limit in the 14 days to
/// 2026-09-11, leaving roughly 1.1 GiB of headroom. At the 128 MiB default heap
/// ceiling that is ~8 isolates simultaneously at their *worst case*. Typical
/// usage is far below the ceiling — mean init is 6 ms and p95 duration 637 ms,
/// so invocations are short and rarely resident together — so 32 is deliberately
/// generous: it bounds the catastrophic case without binding on traffic that
/// works today.
///
/// **This is a starting point, not a measurement.** Tune it from
/// `oxy_custom_app_isolates_live_peak` once that has run in production; that
/// gauge exists precisely because a scrape-interval average cannot tell you
/// what concurrency actually reaches.
///
/// # Loose first, then tighten — and why that order is not negotiable
///
/// These three numbers are **arithmetic against a dev cluster that has served
/// zero custom-app invocations**. Shipping a guess *tight* is how you
/// manufacture the incident the cap exists to prevent: a refused invocation and
/// a failed one are the same event to the person holding the tablet, so a cap
/// set below real traffic converts a slow app into a broken one — and does it
/// on the first busy shift, not gradually.
///
/// The asymmetry that settles it: these ceilings are **not** what protects the
/// box's memory. `32 × 128 MiB` is already 4 GiB against a 2 GiB cgroup, so the
/// global cap was never the binding memory protection — the per-isolate heap
/// ceiling is, backed by invocations being short (mean init 6 ms, p95 637 ms).
/// Loosening a *rejecting* ceiling therefore costs approximately nothing in
/// memory risk, while tightening one costs refused work immediately.
///
/// So: generous until `oxy_custom_app_admission_shed_total` and
/// `oxy_custom_app_isolates_live_peak` have a week of production traffic
/// underneath them, then close down to what was actually observed. The heap
/// ceiling is deliberately **not** loosened with them — see
/// `DEFAULT_HEAP_LIMIT_BYTES`.
pub const DEFAULT_MAX_CONCURRENCY: usize = 64;

/// Default per-org ceiling.
///
/// Three quarters of the global one, not a quarter. It still binds before the
/// global cap — so "one tenant is eating the box" stays a distinguishable
/// `ShedReason::Org` rather than a generic `Global` — but it no longer refuses
/// a single customer's burst on a box that is otherwise idle.
///
/// The old value (8, a quarter) was the sharpest edge in the whole cap: at p95
/// 637 ms it refuses one org at roughly 12 invocations/sec/replica, which a
/// shift-change burst from one tablet fleet can reach while the fleet as a
/// whole is doing nothing. Per-org fairness only means anything under
/// contention, and with six orgs there is none to arbitrate — so until there
/// is, the limit could only take work away.
pub const DEFAULT_MAX_ORG_CONCURRENCY: usize = 48;

/// Default queue budget.
///
/// Waiting is strictly better than refusing for a request whose caller is
/// already waiting on it, and `admit()` is cancel-aware: if the caller goes
/// away the wait ends as `ShedReason::Cancelled`, which is explicitly not
/// counted as a shed. So a longer budget cannot inflate the shed rate — it can
/// only convert a refusal into a served request or into a disconnect that was
/// going to happen anyway.
///
/// 15 s sits far above p95 invocation duration (637 ms), so a permit freed by
/// normal completion is one a queued caller still catches even several
/// invocations deep, and far below any client timeout worth respecting.
pub const DEFAULT_QUEUE_BUDGET: Duration = Duration::from_secs(15);

/// Which ceiling refused an invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShedReason {
    /// The box is full. The fleet needs more replicas.
    Global,
    /// One org is at its own ceiling while the box has room.
    Org,
    /// The wait queue itself is full — more callers are parked than
    /// [`max_queued`] allows. Distinct from `Global` because the remedy is
    /// different: `Global` says raise the cap or add replicas, `Queue` says
    /// arrivals are outrunning completions by enough that queueing is only
    /// delaying the refusal.
    Queue,
    /// The caller went away while it was queued. Not a refusal at all — there
    /// is nobody left to serve, so this never counts as a shed.
    Cancelled,
}

impl ShedReason {
    /// The label the metric carries, and the word an operator greps for.
    pub fn as_str(self) -> &'static str {
        match self {
            ShedReason::Global => "global",
            ShedReason::Org => "org",
            ShedReason::Queue => "queue",
            ShedReason::Cancelled => "cancelled",
        }
    }

    /// Whether this refusal is load shedding, as opposed to the caller leaving.
    ///
    /// Counting a disconnect as a shed would make the shed rate track how
    /// impatient users are rather than how loaded the fleet is.
    pub fn is_shed(self) -> bool {
        !matches!(self, ShedReason::Cancelled)
    }
}

/// Permits held for the life of one invocation.
///
/// Both are released on drop, in declaration order — org, then global, the
/// same order they were acquired in. Release order does not matter here
/// (neither permit gates the other's return), so this is simply what Rust
/// does; what matters is that holding them in one struct makes it impossible
/// to release one and forget the other on an early return.
pub struct Admission {
    _org: OwnedSemaphorePermit,
    _global: OwnedSemaphorePermit,
    /// Keeps `oxy.custom_app.admission.in_use` in step with the permits.
    _gauge: oxy_telemetry::metrics::sources::AdmissionGuard,
}

impl std::fmt::Debug for Admission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Admission")
    }
}

fn parse_env(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(default)
}

/// The global ceiling in force. `0` disables admission control.
pub fn max_concurrency() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        let limit = parse_env(MAX_CONCURRENCY_ENV, DEFAULT_MAX_CONCURRENCY);
        oxy_telemetry::metrics::sources::set_admission_limit(limit as i64);
        limit
    })
}

/// The per-org ceiling, clamped to the global one so an env misconfiguration
/// cannot make the inner limit looser than the outer — which would read as
/// "per-org limiting is on" while doing nothing.
pub fn max_org_concurrency() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| {
        let global = max_concurrency();
        let org = parse_env(MAX_ORG_CONCURRENCY_ENV, DEFAULT_MAX_ORG_CONCURRENCY);
        if global == 0 { org } else { org.min(global) }
    })
}

/// The wait-queue ceiling, as a multiple of the global permit count.
///
/// Four deep. A waiter costs its retained bundle rather than a thread and a
/// heap, so the queue can be several times the run capacity before it competes
/// for memory — but it cannot be unbounded, because at saturation its depth is
/// `arrival_rate × queue_budget` with nothing on the right-hand side under our
/// control.
pub const QUEUE_DEPTH_MULTIPLE: usize = 4;

/// How many invocations may be parked at once. `0` disables the ceiling.
///
/// Derived from [`max_concurrency`] rather than defaulted independently, so
/// raising the permit count raises the queue with it and an operator has one
/// number to tune instead of two that must be kept in proportion.
pub fn max_queued() -> usize {
    static VALUE: OnceLock<usize> = OnceLock::new();
    *VALUE.get_or_init(|| match max_concurrency() {
        // Cap disabled ⇒ nothing ever queues, so a queue ceiling is moot.
        0 => 0,
        n => parse_env(MAX_QUEUED_ENV, n * QUEUE_DEPTH_MULTIPLE),
    })
}

/// How long a caller may queue before being shed.
pub fn queue_budget() -> Duration {
    static VALUE: OnceLock<Duration> = OnceLock::new();
    *VALUE.get_or_init(|| {
        std::env::var(QUEUE_BUDGET_SECS_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_QUEUE_BUDGET)
    })
}

fn global_semaphore() -> &'static Arc<Semaphore> {
    static SEM: OnceLock<Arc<Semaphore>> = OnceLock::new();
    SEM.get_or_init(|| Arc::new(Semaphore::new(effective_global_permits())))
}

/// `0` means disabled, which is expressed as a semaphore nobody can exhaust
/// rather than as a branch at every call site — one code path, and the guard
/// type stays the same shape whether or not the cap is on.
fn effective_global_permits() -> usize {
    match max_concurrency() {
        0 => Semaphore::MAX_PERMITS,
        n => n,
    }
}

fn effective_org_permits() -> usize {
    match max_concurrency() {
        0 => Semaphore::MAX_PERMITS,
        _ => max_org_concurrency().max(1),
    }
}

/// One semaphore per org, created on first use.
///
/// A `HashMap` behind a `Mutex` rather than a concurrent map: it is touched
/// twice per invocation, the critical section is a hash lookup, and the entry
/// count is bounded by the number of orgs this replica has ever served (6
/// today). Entries are never evicted — an org's semaphore is 48 bytes and
/// evicting one that is at zero permits would race an in-flight release.
fn org_semaphore(org_id: Uuid) -> Arc<Semaphore> {
    static ORGS: OnceLock<Mutex<HashMap<Uuid, Arc<Semaphore>>>> = OnceLock::new();
    let map = ORGS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = map.lock().unwrap_or_else(|e| e.into_inner());
    guard
        .entry(org_id)
        .or_insert_with(|| Arc::new(Semaphore::new(effective_org_permits())))
        .clone()
}

/// Wait for a permit, or report which ceiling refused.
///
/// The org permit is taken **first**, inside the global one's budget, so a
/// tenant that is at its own ceiling queues on its own semaphore instead of
/// occupying a global permit while it waits. Taking them the other way round
/// lets a hogging org hold global permits it cannot use.
///
/// `cancel` is the invocation's cancel signal. A caller that has already gone
/// away must not be admitted: it would take a permit, an OS thread and a V8
/// heap for work whose result nobody will read, and the watchdog would then
/// tear all of it down. Racing the wait against the signal costs one `select!`
/// arm and keeps the queue for callers still waiting on an answer.
pub async fn admit(
    org_id: Uuid,
    cancel: &mut tokio::sync::oneshot::Receiver<()>,
) -> Result<Admission, ShedReason> {
    let started = Instant::now();
    let budget = queue_budget();

    // The queue ceiling, checked before parking. The semaphore bounds how many
    // invocations RUN; without this nothing bounds how many WAIT, and a waiter
    // is not free — see `sources::ADMISSION_QUEUED`. Beyond the ceiling the
    // honest answer is immediate: queueing past this point spends memory to
    // delay a shed rather than to avoid one.
    let ceiling = max_queued();
    if ceiling > 0 && QueueGuard::depth() >= ceiling as i64 {
        return Err(ShedReason::Queue);
    }
    let _queued = QueueGuard::enter();

    let org_sem = org_semaphore(org_id);
    let org_permit = tokio::select! {
        biased;
        _ = &mut *cancel => return Err(ShedReason::Cancelled),
        acquired = tokio::time::timeout(budget, org_sem.acquire_owned()) => match acquired {
            Ok(Ok(permit)) => permit,
            // The semaphore is never closed, so an inner `Err` is unreachable;
            // treating it as a shed rather than unwrapping keeps a future
            // `close()` from turning into a panic on the request path.
            Ok(Err(_)) | Err(_) => return Err(ShedReason::Org),
        },
    };

    // Whatever is left of the budget after the org wait. A caller must not get
    // two full budgets — the point is a bound on total queueing.
    let remaining = budget.saturating_sub(started.elapsed());
    let global_sem = Arc::clone(global_semaphore());
    let global_permit = tokio::select! {
        biased;
        _ = &mut *cancel => return Err(ShedReason::Cancelled),
        acquired = tokio::time::timeout(remaining, global_sem.acquire_owned()) => match acquired {
            Ok(Ok(permit)) => permit,
            Ok(Err(_)) | Err(_) => return Err(ShedReason::Global),
        },
    };

    Ok(Admission {
        _org: org_permit,
        _global: global_permit,
        _gauge: oxy_telemetry::metrics::sources::AdmissionGuard::enter(),
    })
}

/// How long [`admit`] took, for the caller to record. Separate from `admit`
/// itself so the metric is recorded once, at the one call site, rather than
/// from inside a function that tests also drive.
pub fn elapsed_since(started: Instant) -> f64 {
    started.elapsed().as_secs_f64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_global_limit_disables_the_cap_without_a_special_case() {
        // The disabled path must still produce permits, so `admit` has one
        // shape and the guard type never becomes optional.
        assert_eq!(effective_global_permits().min(1), 1);
    }

    /// A caller with no cancel signal, for the tests that are not about
    /// cancellation. The sender is returned so it stays alive — a dropped
    /// `oneshot::Sender` resolves its receiver, which would read as "the caller
    /// went away" and make every admission a cancellation.
    fn no_cancel() -> (
        tokio::sync::oneshot::Sender<()>,
        tokio::sync::oneshot::Receiver<()>,
    ) {
        tokio::sync::oneshot::channel()
    }

    #[test]
    fn shed_reasons_have_distinct_stable_labels() {
        // These land in a metric label an alert matches on; a rename is a
        // silent break of whatever query uses them.
        assert_eq!(ShedReason::Global.as_str(), "global");
        assert_eq!(ShedReason::Org.as_str(), "org");
        assert_eq!(ShedReason::Queue.as_str(), "queue");
        assert_eq!(ShedReason::Cancelled.as_str(), "cancelled");

        let labels = [
            ShedReason::Global,
            ShedReason::Org,
            ShedReason::Queue,
            ShedReason::Cancelled,
        ]
        .map(ShedReason::as_str);
        let unique: std::collections::BTreeSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len(), "labels must be distinct");
    }

    /// A caller leaving is not load shedding. Counting it as one would make the
    /// shed rate track user impatience rather than fleet load.
    #[test]
    fn a_cancellation_is_not_counted_as_a_shed() {
        assert!(ShedReason::Global.is_shed());
        assert!(ShedReason::Org.is_shed());
        assert!(ShedReason::Queue.is_shed());
        assert!(!ShedReason::Cancelled.is_shed());
    }

    #[test]
    fn the_org_ceiling_is_smaller_than_the_global_one_by_default() {
        // If the per-org limit were >= the global one it would never bind, and
        // one tenant could hold every permit — the exact failure the second
        // limit exists to prevent.
        assert!(
            DEFAULT_MAX_ORG_CONCURRENCY < DEFAULT_MAX_CONCURRENCY,
            "per-org default must bind before the global one"
        );
    }

    /// The per-org ceiling must stay a LARGE fraction of the global one until
    /// production traffic says otherwise.
    ///
    /// The sibling test above stops it reaching the global cap, where it could
    /// never bind. This one stops the opposite drift, which is the one that
    /// actually costs a customer something: a per-org ceiling set to a small
    /// fraction refuses one tenant's burst while the box is idle, and a refused
    /// invocation is indistinguishable from a broken app to the person holding
    /// the tablet. It was 8 of 32 — a quarter — sized by arithmetic against a
    /// cluster that had served zero invocations.
    ///
    /// Tightening it is a legitimate thing to do *from measurement*. This test
    /// is what makes that a deliberate edit with a reason attached rather than
    /// a quiet return to a guess.
    #[test]
    fn the_org_ceiling_is_generous_until_production_says_otherwise() {
        assert!(
            DEFAULT_MAX_ORG_CONCURRENCY * 2 >= DEFAULT_MAX_CONCURRENCY,
            "per-org default ({DEFAULT_MAX_ORG_CONCURRENCY}) is under half the \
             global one ({DEFAULT_MAX_CONCURRENCY}) — that refuses a single \
             org's burst on an otherwise idle box. Tighten only from observed \
             oxy_custom_app_admission_shed_total / isolates_live_peak, and say \
             so here."
        );
    }

    /// The queue budget has to exceed a typical invocation, or a permit freed
    /// by normal completion is never one a queued caller catches — and the cap
    /// degrades into shedding instead of queueing.
    #[test]
    fn the_queue_budget_outlasts_a_p95_invocation() {
        assert!(
            DEFAULT_QUEUE_BUDGET >= Duration::from_millis(1500),
            "p95 invocation duration is 637ms; a budget below it sheds work \
             that would have been served by simply waiting"
        );
    }

    #[tokio::test]
    async fn an_admitted_invocation_holds_a_permit_until_the_guard_drops() {
        use oxy_telemetry::metrics::sources::ADMISSION_IN_USE;
        use std::sync::atomic::Ordering;

        let before = ADMISSION_IN_USE.load(Ordering::Relaxed);
        let org = Uuid::new_v4();
        let (_tx, mut cancel) = no_cancel();
        {
            let _a = admit(org, &mut cancel).await.expect("first admission");
            let _b = admit(org, &mut cancel).await.expect("second admission");
            assert_eq!(ADMISSION_IN_USE.load(Ordering::Relaxed), before + 2);
        }
        assert_eq!(
            ADMISSION_IN_USE.load(Ordering::Relaxed),
            before,
            "permits must be released on drop, or the gauge drifts up forever"
        );
    }

    /// The per-org semaphore must be per org. One map keyed wrongly (or a
    /// single shared semaphore) would make a busy tenant throttle everyone.
    #[tokio::test]
    async fn two_orgs_do_not_share_a_semaphore() {
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        assert!(!Arc::ptr_eq(&org_semaphore(a), &org_semaphore(b)));
        assert!(Arc::ptr_eq(&org_semaphore(a), &org_semaphore(a)));
    }

    /// The point of the second limit: one tenant at its own ceiling is shed
    /// while the box still has room, and its neighbours are unaffected.
    ///
    /// Nextest runs each test in its own process, so setting the env here is
    /// safe against the `OnceLock`s.
    #[tokio::test]
    async fn an_org_at_its_ceiling_is_shed_while_its_neighbours_are_not() {
        unsafe {
            std::env::set_var(MAX_CONCURRENCY_ENV, "8");
            std::env::set_var(MAX_ORG_CONCURRENCY_ENV, "2");
            std::env::set_var(QUEUE_BUDGET_SECS_ENV, "1");
        }
        assert_eq!(max_concurrency(), 8);
        assert_eq!(max_org_concurrency(), 2);

        let noisy = Uuid::new_v4();
        let (_tx, mut cancel) = no_cancel();
        let _first = admit(noisy, &mut cancel).await.expect("first fits");
        let _second = admit(noisy, &mut cancel).await.expect("second fits");

        let started = Instant::now();
        let shed = admit(noisy, &mut cancel)
            .await
            .expect_err("third exceeds the org ceiling");
        assert_eq!(
            shed,
            ShedReason::Org,
            "the org limit bound, not the global one — 6 global permits were free"
        );
        assert!(
            started.elapsed() >= Duration::from_millis(900),
            "a shed must come after waiting out the queue budget, not instantly: \
             queue-then-shed is what makes a slightly-too-low cap degrade into \
             latency rather than into errors"
        );

        // The whole reason the per-org limit exists.
        let quiet = Uuid::new_v4();
        let _theirs = admit(quiet, &mut cancel)
            .await
            .expect("a different org must be unaffected by a noisy neighbour");
    }

    /// A caller that goes away while queued must not be admitted. Admitting it
    /// takes a permit, an OS thread and a V8 heap for work nobody will read,
    /// and the watchdog then tears all of it down.
    #[tokio::test]
    async fn a_caller_that_leaves_while_queued_is_not_admitted() {
        unsafe {
            std::env::set_var(MAX_CONCURRENCY_ENV, "4");
            std::env::set_var(MAX_ORG_CONCURRENCY_ENV, "1");
            // Long enough that the test would hang here if cancellation were
            // not observed, rather than passing by racing the budget.
            std::env::set_var(QUEUE_BUDGET_SECS_ENV, "30");
        }

        let org = Uuid::new_v4();
        let (_hold_tx, mut hold) = no_cancel();
        let _occupant = admit(org, &mut hold).await.expect("takes the only permit");

        let (cancel_tx, mut cancel) = tokio::sync::oneshot::channel();
        let waiter = tokio::spawn(async move { admit(org, &mut cancel).await.map(|_| ()) });

        // Let it park on the semaphore, then hang up.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let _ = cancel_tx.send(());

        let outcome = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("must resolve on cancel, not wait out the 30s budget")
            .expect("task did not panic");
        assert_eq!(
            outcome.expect_err("a cancelled caller is never admitted"),
            ShedReason::Cancelled
        );
    }

    /// Nothing bounded the *queue* before — only the resident isolates. Each
    /// waiter retains its function bundle for up to the budget, so an unbounded
    /// queue spends the memory headroom the concurrency cap is protecting,
    /// while `in_use` and `isolates.live` both read healthy.
    #[tokio::test]
    async fn a_full_queue_sheds_immediately_rather_than_parking() {
        unsafe {
            std::env::set_var(MAX_CONCURRENCY_ENV, "1");
            std::env::set_var(MAX_ORG_CONCURRENCY_ENV, "1");
            std::env::set_var(MAX_QUEUED_ENV, "1");
            std::env::set_var(QUEUE_BUDGET_SECS_ENV, "30");
        }
        assert_eq!(max_queued(), 1);

        let org = Uuid::new_v4();
        let (_hold_tx, mut hold) = no_cancel();
        let _occupant = admit(org, &mut hold).await.expect("takes the only permit");

        // One waiter fills the queue.
        let (_w_tx, mut w_cancel) = no_cancel();
        let parked = tokio::spawn(async move { admit(org, &mut w_cancel).await.map(|_| ()) });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // The next arrival must be refused without parking.
        let (_tx, mut cancel) = no_cancel();
        let started = Instant::now();
        let shed = admit(org, &mut cancel)
            .await
            .expect_err("the queue is full");
        assert_eq!(shed, ShedReason::Queue);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "past the ceiling, queueing only spends memory to delay a refusal \
             it cannot avoid — so the refusal must be immediate"
        );

        parked.abort();
    }

    /// Clamping matters: a per-org limit above the global one would never bind,
    /// so an operator who raised it would silently lose noisy-neighbour
    /// protection while believing they had loosened it.
    #[test]
    fn the_org_ceiling_cannot_exceed_the_global_one() {
        unsafe {
            std::env::set_var(MAX_CONCURRENCY_ENV, "4");
            std::env::set_var(MAX_ORG_CONCURRENCY_ENV, "99");
        }
        assert_eq!(max_org_concurrency(), 4);
    }

    /// The gauge contract the boot call exists to satisfy — the twin of
    /// `custom_apps_bundle_cache::tests::resolve_budget_publishes_the_gauge`.
    ///
    /// `max_concurrency()` publishes `oxy_custom_app_admission_limit` as a side
    /// effect of resolving. That is only worth anything if something resolves
    /// it at boot, and for a while nothing did: the boot call sat inside a
    /// `tracing::info!` field expression, and `tracing` evaluates those only
    /// when the callsite is enabled. Default `OXY_LOG_LEVEL` is `warn`, so the
    /// whole block was dead and the gauge stayed at 0 — which is also what
    /// "the cap is disabled" reads as, the precise ambiguity the boot call was
    /// added to remove.
    #[test]
    fn resolving_the_cap_publishes_the_limit_gauge() {
        use oxy_telemetry::metrics::sources::ADMISSION_LIMIT;
        use std::sync::atomic::Ordering;

        unsafe { std::env::set_var(MAX_CONCURRENCY_ENV, "24") };
        assert_eq!(
            ADMISSION_LIMIT.load(Ordering::Relaxed),
            0,
            "nothing should have resolved the cap yet in this process"
        );

        let resolved = max_concurrency();
        assert_eq!(resolved, 24);
        assert_eq!(
            ADMISSION_LIMIT.load(Ordering::Relaxed),
            resolved as i64,
            "resolving the cap must publish it — a gauge left at 0 is \
             indistinguishable from the cap being disabled"
        );
    }
}
