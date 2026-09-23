//! Process-local numbers that owners publish and the observable instruments read.
//!
//! An observable gauge needs its value *pulled* at collection time, which means
//! the callback has to reach whatever owns the number. The owners here are in
//! other crates — the Postgres pool in `oxy-platform`, the V8 isolate count in
//! `oxy-app` — and a callback registry that let them hand in closures would
//! need `'static` closures holding those owners alive for the process lifetime.
//!
//! Plain atomics avoid all of it. The owner `store`s on the cadence it already
//! has (the pool monitor already wakes every `POOL_HEALTH_INTERVAL`; the isolate
//! count changes on spawn and exit), the callback `load`s. No registry, no
//! lifetimes, no lock on the collection path.
//!
//! **Every value here is per-process**, which is the same caveat
//! `worker_metrics` documents at length for its reap counters: a fleet of N
//! replicas reports N series, and a dashboard that sums them is answering a
//! different question than one that takes `max`. The per-metric guidance lives
//! with each gauge in [`super::instruments`].
//!
//! Absent-vs-zero: these are all deliberately *zero* rather than absent before
//! anything publishes, because zero is the truth for every one of them — no
//! pool yet means no connections, no isolate yet means none live. That is the
//! opposite of the `oxy_router_last_probe_received_timestamp_seconds` case,
//! where zero would read as 1970. Keep the distinction when adding to this list.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

/// Connections the pool currently holds open (idle + in use).
pub static DB_POOL_SIZE: AtomicU64 = AtomicU64::new(0);

/// Connections the pool holds that are not checked out.
pub static DB_POOL_IDLE: AtomicU64 = AtomicU64::new(0);

/// The pool's configured ceiling — `OXY_DATABASE_MAX_CONNECTIONS`, or the
/// default. Published once at pool construction so the saturation ratio
/// `(size - idle) / max` can be computed entirely from exported series.
pub static DB_POOL_MAX: AtomicU64 = AtomicU64::new(0);

/// Whether the pool's own health probe last failed to acquire a connection.
///
/// This is the signal behind the "database connection pool is starved"
/// `ERROR`, exported so an alert can fire on the condition rather than on a
/// log-line pattern. Prod reached this state and served 30s stalls while the
/// only evidence was a log line nothing was matching on.
pub static DB_POOL_STARVED: AtomicBool = AtomicBool::new(false);

/// How many times the pool's health probe has failed to acquire.
///
/// Monotonic and per-process. The gauge above says "right now"; this says "has
/// it been happening", which is what distinguishes a blip from a pattern
/// between two scrapes.
pub static DB_POOL_STARVATION_EVENTS: AtomicU64 = AtomicU64::new(0);

/// V8 isolates alive in this process right now.
///
/// One invocation is one isolate on one OS thread, so this is simultaneously
/// the isolate count, the function concurrency and the count of threads
/// outside Tokio's pool. **Nothing bounds it today** — there is no semaphore in
/// `custom_apps_functions/`, and the `cpu: 2` limit sizes Tokio's workers, not
/// these. This gauge is the measurement that has to exist before a cap can be
/// sized, and its high-water mark across the fleet is the input to both the
/// concurrency limit and the per-isolate heap ceiling (the two only make sense
/// together: heap ceiling × concurrency cap ≤ the pod's memory budget).
pub static ISOLATES_LIVE: AtomicI64 = AtomicI64::new(0);

/// High-water mark of [`ISOLATES_LIVE`] since process start.
///
/// A gauge scraped every 30s misses a spike that opens and closes between two
/// scrapes, and the spike is exactly what the cap has to be sized against. This
/// never decreases, so `max` over the fleet is the number to size from.
pub static ISOLATES_LIVE_PEAK: AtomicI64 = AtomicI64::new(0);

/// Raise [`ISOLATES_LIVE`] and keep [`ISOLATES_LIVE_PEAK`] up to date.
pub fn isolate_started() {
    let live = ISOLATES_LIVE.fetch_add(1, Ordering::Relaxed) + 1;
    // Compare-and-swap rather than a plain store: two threads starting at once
    // would otherwise let the lower of the two win and understate the peak.
    let mut peak = ISOLATES_LIVE_PEAK.load(Ordering::Relaxed);
    while live > peak {
        match ISOLATES_LIVE_PEAK.compare_exchange_weak(
            peak,
            live,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => peak = observed,
        }
    }
}

/// Lower [`ISOLATES_LIVE`]. Pairs with [`isolate_started`].
pub fn isolate_finished() {
    ISOLATES_LIVE.fetch_sub(1, Ordering::Relaxed);
}

/// Isolate threads detached after their termination grace period expired.
///
/// The home of the count that `oxy-app`'s
/// `custom_apps_functions::runtime::abandoned_isolates()` reads. It lives here
/// rather than in that module so the value is reachable from the metrics layer
/// on **every** role — the counter was previously a private static that only
/// `worker_metrics` read, so it was exported by `oxy worker` alone. The worker
/// does create isolates (it claims scheduled and job-mode `app_function`
/// tasks), but **route-mode** invocations run on `oxy serve`, which exported
/// nothing — so the majority of them were uncounted.
pub static ISOLATES_ABANDONED: AtomicU64 = AtomicU64::new(0);

/// Count one abandoned isolate thread.
pub fn isolate_abandoned() -> u64 {
    ISOLATES_ABANDONED.fetch_add(1, Ordering::Relaxed) + 1
}

/// Threads abandoned so far on this process. Healthy is zero.
pub fn abandoned_isolates() -> u64 {
    ISOLATES_ABANDONED.load(Ordering::Relaxed)
}

/// Raises [`ISOLATES_LIVE`] for as long as it is held.
///
/// An RAII pair rather than two bare calls, because the isolate thread has
/// several exit paths — an early `return` when its Tokio runtime fails to
/// build, a panic inside tenant code, and the normal end — and a missed
/// decrement is a gauge that drifts upward forever.
///
/// **An abandoned thread keeps its guard**, so it keeps counting as live. That
/// is correct rather than a leak in the accounting: a detached isolate is still
/// holding a thread and a heap, and the number this gauge exists to inform — how
/// much memory concurrent isolates can demand — has to include it.
pub struct IsolateGuard(());

impl IsolateGuard {
    /// Count one isolate as live until the returned guard drops.
    pub fn enter() -> Self {
        isolate_started();
        Self(())
    }
}

impl Drop for IsolateGuard {
    fn drop(&mut self) {
        isolate_finished();
    }
}

/// Admission permits currently held by running invocations.
///
/// The numerator of the saturation ratio for the function concurrency cap.
/// Distinct from [`ISOLATES_LIVE`] and the gap between them is informative: a
/// permit is released when the invocation returns, but an **abandoned** isolate
/// keeps its thread and heap without holding a permit. `live > in_use` is
/// therefore the count of isolates the cap is no longer accounting for, which
/// is exactly the leak `ISOLATES_ABANDONED` counts.
pub static ADMISSION_IN_USE: AtomicI64 = AtomicI64::new(0);

/// The configured global permit ceiling, published so the saturation ratio
/// needs no out-of-band constant. Zero means the cap is disabled.
pub static ADMISSION_LIMIT: AtomicI64 = AtomicI64::new(0);

/// Bytes the custom-app bundle cache currently holds.
///
/// The cache was bounded by entry *count* only, which set its ceiling from
/// what tenants publish rather than from anything we chose — 8192 slots × a
/// multi-MiB chunk is GiBs, with the pod's cgroup limit as the only backstop.
/// It is now byte-bounded, and this is what says whether the budget is being
/// approached, together with [`BUNDLE_CACHE_LIMIT`] as its denominator.
pub static BUNDLE_CACHE_BYTES: AtomicU64 = AtomicU64::new(0);

/// The configured resident-byte budget; `0` when the byte bound is disabled.
///
/// **Published at boot, by `custom_apps_bundle_cache::resolve_budget()` from
/// serve startup — not lazily.** The distinction is the whole value of the
/// gauge: left to be resolved by the first custom-app asset request, this reads
/// `0` until then, and `0` is also what "the bound is disabled" looks like. Over
/// that window the two are indistinguishable and the saturation ratio this is
/// the denominator of divides by zero.
///
/// Same rule as [`set_admission_limit`], for the same reason, and this one
/// reintroduced the defect once before it was caught in review.
pub static BUNDLE_CACHE_LIMIT: AtomicU64 = AtomicU64::new(0);

/// Publish the bundle cache's current resident size.
pub fn set_bundle_cache_bytes(bytes: u64) {
    BUNDLE_CACHE_BYTES.store(bytes, Ordering::Relaxed);
}

/// Publish the bundle cache's configured budget.
pub fn set_bundle_cache_limit(bytes: u64) {
    BUNDLE_CACHE_LIMIT.store(bytes, Ordering::Relaxed);
}

/// Publish the admission cap's ceiling once it is known.
///
/// **Call this at boot, not lazily.** Published only as a side effect of the
/// first invocation, this gauge reads `0` — the documented value for "the cap
/// is disabled" — for the whole window between a replica starting and its first
/// function call, which on most replicas is not short. The saturation ratio it
/// is the denominator of would divide by zero over exactly that window.
pub fn set_admission_limit(limit: i64) {
    ADMISSION_LIMIT.store(limit, Ordering::Relaxed);
}

/// Invocations waiting for a concurrency permit right now.
///
/// The semaphore bounds how many invocations *run*; this is how many are
/// parked. It matters because a waiter is not free: admission happens inside
/// `runtime::run`, after the function bundle, the invocation context and the
/// resolved host have been moved in, so each waiter retains all of that for up
/// to the queue budget. Under saturation that is `arrival_rate × budget` live
/// bundles, eating the same headroom the concurrency arithmetic is spending —
/// while `admission.in_use` and `isolates.live` both read perfectly healthy.
///
/// Without this series that state is invisible until it is an outage.
pub static ADMISSION_QUEUED: AtomicI64 = AtomicI64::new(0);

/// Raises [`ADMISSION_QUEUED`] for as long as it is held.
pub struct QueueGuard(());

impl QueueGuard {
    /// Count one invocation as queued until the returned guard drops.
    pub fn enter() -> Self {
        ADMISSION_QUEUED.fetch_add(1, Ordering::Relaxed);
        Self(())
    }

    /// How many are queued right now, for a ceiling check.
    pub fn depth() -> i64 {
        ADMISSION_QUEUED.load(Ordering::Relaxed)
    }
}

impl Drop for QueueGuard {
    fn drop(&mut self) {
        ADMISSION_QUEUED.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Raises [`ADMISSION_IN_USE`] for as long as it is held.
///
/// Separate from [`IsolateGuard`] on purpose. This one covers the whole
/// invocation — including the setup before an isolate exists and the teardown
/// after it is gone — because that is the window a permit is actually occupied
/// for. Conflating the two would understate saturation by however long setup
/// takes.
pub struct AdmissionGuard(());

impl AdmissionGuard {
    /// Count one permit as held until the returned guard drops.
    pub fn enter() -> Self {
        ADMISSION_IN_USE.fetch_add(1, Ordering::Relaxed);
        Self(())
    }
}

impl Drop for AdmissionGuard {
    fn drop(&mut self) {
        ADMISSION_IN_USE.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Publish the pool's current occupancy. Called from the pool health monitor.
pub fn set_db_pool(size: u64, idle: u64, max: u64) {
    DB_POOL_SIZE.store(size, Ordering::Relaxed);
    DB_POOL_IDLE.store(idle, Ordering::Relaxed);
    DB_POOL_MAX.store(max, Ordering::Relaxed);
}

/// Publish whether the pool is currently starved, counting the transition into
/// starvation so a between-scrapes blip still leaves evidence.
pub fn set_db_pool_starved(starved: bool) {
    let was = DB_POOL_STARVED.swap(starved, Ordering::Relaxed);
    if starved && !was {
        DB_POOL_STARVATION_EVENTS.fetch_add(1, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The peak must survive the live count coming back down — it is the only
    /// record of a spike that opened and closed between two scrapes, which is
    /// the case the concurrency cap has to be sized against.
    #[test]
    fn isolate_peak_holds_after_the_live_count_falls() {
        ISOLATES_LIVE.store(0, Ordering::Relaxed);
        ISOLATES_LIVE_PEAK.store(0, Ordering::Relaxed);

        isolate_started();
        isolate_started();
        isolate_started();
        assert_eq!(ISOLATES_LIVE.load(Ordering::Relaxed), 3);
        assert_eq!(ISOLATES_LIVE_PEAK.load(Ordering::Relaxed), 3);

        isolate_finished();
        isolate_finished();
        isolate_finished();
        assert_eq!(ISOLATES_LIVE.load(Ordering::Relaxed), 0);
        assert_eq!(
            ISOLATES_LIVE_PEAK.load(Ordering::Relaxed),
            3,
            "the peak is the whole point of the second static"
        );
    }

    /// Only the *transition* into starvation counts. A pool that stays starved
    /// across ten probes is one incident, not ten — otherwise the counter
    /// measures probe cadence rather than how often the pool broke.
    #[test]
    fn starvation_events_count_transitions_not_probes() {
        DB_POOL_STARVED.store(false, Ordering::Relaxed);
        DB_POOL_STARVATION_EVENTS.store(0, Ordering::Relaxed);

        set_db_pool_starved(true);
        set_db_pool_starved(true);
        set_db_pool_starved(true);
        assert_eq!(DB_POOL_STARVATION_EVENTS.load(Ordering::Relaxed), 1);

        set_db_pool_starved(false);
        set_db_pool_starved(true);
        assert_eq!(
            DB_POOL_STARVATION_EVENTS.load(Ordering::Relaxed),
            2,
            "recovering and breaking again is a second incident"
        );
    }
}
