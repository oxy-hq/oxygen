//! What bounds one isolate: its heap ceiling, how long a terminated one is
//! waited for, and the count of the ones that were not.

/// Threads abandoned so far on this process. Healthy is zero.
///
/// The detach is deliberate (see the grace arm in [`run`]) and it is also an
/// unbounded resource leak. A rising value is the earliest available signal
/// that a tenant's function is wedged in a host call that will not return, and
/// it is a fact about **our process** rather than about the app — so it belongs
/// in platform telemetry, never in the tenant-facing store.
///
/// The count itself lives in `oxy_telemetry::metrics::sources`, not here.
/// It used to be a private static in this module that only `worker_metrics`
/// read, and `worker_metrics` is mounted only on `oxy worker`'s health port.
/// The worker does claim `app_function` tasks (scheduled and job-mode runs), so
/// it creates isolates and the counter was not inert there — but **route-mode**
/// invocations, which are the bulk, run on `oxy serve`, which published
/// nothing. Moving the storage into the telemetry crate is what lets serve
/// export it too.
///
/// Scraped as `oxy_abandoned_isolates_total` (hand-rolled, `worker_metrics`)
/// and as `oxy_custom_app_isolates_abandoned_total` (OTel, every role).
/// Deliberately not on the fleet-health API: that endpoint is `FleetOk`, so a
/// load-balanced read would report whichever replica answered.
pub fn abandoned_isolates() -> u64 {
    oxy_telemetry::metrics::sources::abandoned_isolates()
}

/// Per-isolate heap ceiling, in bytes. Override with
/// [`HEAP_LIMIT_MB_ENV`]; `0` disables the ceiling.
///
/// 128 MiB is Cloudflare Workers' per-isolate limit. Matching a published
/// number is itself the argument: an app author who hits it can look up what it
/// means and what to do, which is not true of a number we invented.
///
/// **Why any ceiling is strictly safer than none.** `create_params` was unset,
/// so V8 used its own default — far above the pod's 2 GiB cgroup limit. The
/// kernel OOM killer therefore always won the race, and it kills the *process*,
/// which is serving 44 apps, every org subdomain and every `FleetOk` product
/// route. With a ceiling, the same runaway allocation terminates one isolate
/// and returns an error to one app. The only traffic this newly breaks is a
/// function that legitimately holds >128 MiB of live JS objects — and that
/// function was already one spike away from taking the fleet down.
/// Per-isolate heap ceiling.
///
/// **Deliberately not loosened alongside the admission ceilings.** Those are
/// *rejecting* limits whose failure mode when set too tight is refused work;
/// this one is the *containment* limit, and it is what actually keeps one
/// tenant's runaway allocation from taking the pod — and with it every other
/// tenant on that replica — down. `limits::DEFAULT_MAX_CONCURRENCY × this` is
/// already well past the cgroup limit, which is the proof that the concurrency
/// cap is not the memory protection: this is.
///
/// Its own too-tight failure mode is real but bounded and visible: one
/// invocation is terminated, `oxy_custom_app_isolates_heap_terminations_total`
/// records it against the org, and nothing else on the replica is affected.
/// That is a strictly better trade than a pod OOM, so this number moves only on
/// evidence of legitimate functions hitting it — not as part of the
/// loosen-then-tighten pass on the admission ceilings.
const DEFAULT_HEAP_LIMIT_BYTES: usize = 128 * 1024 * 1024;

/// Override for [`DEFAULT_HEAP_LIMIT_BYTES`], in megabytes. `0` disables.
pub const HEAP_LIMIT_MB_ENV: &str = "OXY_FUNCTION_HEAP_LIMIT_MB";

/// The ceiling in force for this process. `None` disables it.
pub fn heap_limit_bytes() -> Option<usize> {
    static VALUE: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *VALUE.get_or_init(|| {
        match std::env::var(HEAP_LIMIT_MB_ENV)
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
        {
            Some(0) => None,
            Some(mb) => Some(mb * 1024 * 1024),
            None => Some(DEFAULT_HEAP_LIMIT_BYTES),
        }
    })
}
/// How long to wait for the isolate thread to actually exit after a wall-clock
/// timeout (or cancel) has terminated execution, before giving up and returning
/// `Timeout` regardless. Bounds the worst case where the isolate is parked in a
/// not-yet-returned host call that `terminate_execution` can't interrupt.
pub(super) const TIMEOUT_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
