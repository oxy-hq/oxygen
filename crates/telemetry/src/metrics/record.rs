//! Typed recording entry points.
//!
//! Call sites pass plain values — strings, a status, a duration — and never
//! build an attribute vector. That is the point: the label set for a given
//! metric is decided here and nowhere else, so the cardinality discipline in
//! [`super::instruments`] cannot be bypassed by a new call site that happens to
//! add one more attribute.
//!
//! It also keeps `opentelemetry` out of the dependency graph of every crate
//! that wants to record something. `oxy-app` records custom-app timings and has
//! no OTel dependency; it calls these functions instead.

use opentelemetry::KeyValue;

use super::Instruments;
use super::instruments::app_label;
use super::with_instruments;

/// Attribute keys, spelled once. A typo in a label key produces a second
/// series rather than an error, which is the kind of bug that is only found
/// when a dashboard is quietly missing half its data.
const ORG: &str = "oxy.org_id";
const APP: &str = "oxy.app";
const KIND: &str = "oxy.kind";
const FUNCTION: &str = "oxy.function";
const OUTCOME: &str = "oxy.outcome";
const STATUS: &str = "http.response.status_code";
const REASON: &str = "oxy.reason";

/// The two `reason` values [`db_pool_probe_failure`] can carry.
///
/// Public because `oxy-platform`'s probe selects from these rather than
/// spelling its own literals. [`seed_zero_series`] has to seed *the same*
/// series the probe will later increment, and two string literals in two
/// crates drift without a compile error — which would leave a seeded decoy at
/// 0 beside the real series. One definition makes that impossible.
pub const DB_POOL_PROBE_FAILURE_TIMEOUT: &str = "timeout";
/// See [`DB_POOL_PROBE_FAILURE_TIMEOUT`].
pub const DB_POOL_PROBE_FAILURE_ERROR: &str = "error";

const DB_POOL_PROBE_FAILURE_REASONS: [&str; 2] =
    [DB_POOL_PROBE_FAILURE_TIMEOUT, DB_POOL_PROBE_FAILURE_ERROR];

/// Give the synchronous counters an alert can be written against a sample at
/// zero, at install time.
///
/// **Why this is needed at all.** An *observable* instrument's callback runs on
/// every collection, so its series exists from the first scrape and
/// `increase(x[1h]) > 0` sees `0 -> 1` on the first event. A *synchronous*
/// counter is different: OTel exports only instruments that hold data, so the
/// series does not exist until the first `add()` — it is **born carrying that
/// first value, with no earlier sample to subtract from**. `increase()` over it
/// therefore returns 0 for a counter that appears at 1 and never moves again,
/// and the first, isolated occurrence is invisible to an alert. That is the
/// "alarm that looks like coverage but can never fire" failure this whole
/// module exists to close, so the counters that *can* be seeded are.
///
/// **Why only these two.** A seed must carry the **exact** attribute set the
/// real record will use. Seeding a different one is strictly worse than not
/// seeding: it produces a decoy series pinned at 0 while the real one is born
/// beside it, so a rule reads calm precisely when something is happening. That
/// rules out every counter keyed by org — `custom_app_admission_shed`,
/// `custom_app_heap_termination`, `custom_app_function` — because the org set
/// is not known here and is unbounded anyway. Those stay un-seeded, and a rule
/// on them needs the birth clause `(x > 0) unless (x offset 1h)` instead.
///
/// Seeding is also *not* free of meaning: it asserts these series should exist
/// on every process. Both do — the pool probe runs everywhere, and the bundle
/// cache is constructed on any role that can serve a custom app.
pub(super) fn seed_zero_series(i: &Instruments) {
    i.custom_app_bundle_cache_evictions.add(0, &[]);
    for reason in DB_POOL_PROBE_FAILURE_REASONS {
        i.db_pool_probe_failures
            .add(0, &[KeyValue::new(REASON, reason)]);
    }
}

/// One successful pool health probe, and how long it waited for a connection.
pub fn db_pool_probe(seconds: f64) {
    with_instruments(|i| i.db_pool_probe_duration.record(seconds, &[]));
}

/// One pool health probe that did not acquire.
///
/// `reason` is `timeout` (the pool was full — nothing freed within the probe's
/// budget) or `error` (the *server* refused us). They are different incidents:
/// the second is the shape of the prod stall where `max_connections` sat
/// pending-reboot and sqlx swallowed the Postgres `FATAL`, leaving only latency
/// that looked like slow queries.
///
/// Neither records a duration. A timeout never finished; an error finished
/// without acquiring, so its elapsed time measures how fast the server said no
/// rather than how long a checkout takes. So the histogram's `_count` and this
/// counter differ by exactly the probes that did not acquire — read together.
pub fn db_pool_probe_failure(reason: &'static str) {
    with_instruments(|i| {
        i.db_pool_probe_failures
            .add(1, &[KeyValue::new(REASON, reason)]);
    });
}

/// One custom-app serve-plane request — an HTML shell or a bundle asset.
///
/// `kind` is `html` or `asset`: the two have different SLOs (a cancelled image
/// is not an outage, a failed shell is) and folding them would make the shell
/// signal unreadable under asset volume.
pub fn custom_app_request(org_id: &str, app_id: &str, kind: &str, status: u16, seconds: f64) {
    with_instruments(|i| {
        i.custom_app_request_duration.record(
            seconds,
            &[
                KeyValue::new(ORG, org_id.to_owned()),
                KeyValue::new(APP, app_label(app_id).into_owned()),
                KeyValue::new(KIND, kind.to_owned()),
                KeyValue::new(STATUS, i64::from(status)),
            ],
        );
    });
}

/// One Oxy Function invocation.
///
/// `init_seconds` is `None` when tenant code was never reached — a compile
/// failure, or a timeout during setup. Recording a `0` there would claim setup
/// was instantaneous on exactly the invocations where it was not, so the
/// sample is skipped instead.
///
/// The `function` label is carried **only for watchlisted apps**. Function
/// names are author-chosen and unbounded, so `org × app × function × outcome`
/// is the one product here that can actually run away; for everything else the
/// question this answers is "which org's functions are failing", which org and
/// outcome already answer.
pub fn custom_app_function(
    org_id: &str,
    app_id: &str,
    function: &str,
    outcome: &str,
    duration_seconds: f64,
    init_seconds: Option<f64>,
    host_calls: u32,
) {
    let app = app_label(app_id);
    let watchlisted = app != super::instruments::APP_LABEL_OTHER;

    let mut attrs = vec![
        KeyValue::new(ORG, org_id.to_owned()),
        KeyValue::new(APP, app.clone().into_owned()),
        KeyValue::new(OUTCOME, outcome.to_owned()),
    ];
    if watchlisted {
        attrs.push(KeyValue::new(FUNCTION, function.to_owned()));
    }

    with_instruments(|i| {
        i.custom_app_function_duration
            .record(duration_seconds, &attrs);
        i.custom_app_function_invocations.add(1, &attrs);
        i.custom_app_function_host_calls
            .record(f64::from(host_calls), &attrs);
        if let Some(init) = init_seconds {
            i.custom_app_function_init_duration.record(init, &attrs);
        }
    });
}

/// One isolate terminated for breaching its heap ceiling.
///
/// Labelled by org only. The app is deliberately not carried even for a
/// watchlisted app: this is a **host-health** fact — which tenant is taking
/// memory from the box everyone shares — and the org is the unit an operator
/// acts on. The invocation row and its logs are where you then find the app.
pub fn custom_app_heap_termination(org_id: &str) {
    with_instruments(|i| {
        i.custom_app_heap_terminations
            .add(1, &[KeyValue::new(ORG, org_id.to_owned())]);
    });
}

/// How long one invocation waited for a concurrency permit.
///
/// Recorded on **every** admitted invocation, including the uncontended ones
/// that waited microseconds. Recording only the slow waits would make the
/// histogram describe a population that does not exist, and `_count` would stop
/// matching the invocation count.
pub fn custom_app_admission_wait(org_id: &str, seconds: f64) {
    with_instruments(|i| {
        i.custom_app_admission_wait
            .record(seconds, &[KeyValue::new(ORG, org_id.to_owned())]);
    });
}

/// One invocation shed because no permit came free within the queue budget.
///
/// `reason` is `global` or `org` — which limit bound. The distinction is the
/// whole point of having two: `global` means the box is full and the fleet
/// needs more replicas, `org` means one tenant is monopolising it and everyone
/// else is fine.
pub fn custom_app_admission_shed(org_id: &str, reason: &'static str) {
    with_instruments(|i| {
        i.custom_app_admission_shed.add(
            1,
            &[
                KeyValue::new(ORG, org_id.to_owned()),
                KeyValue::new("oxy.reason", reason),
            ],
        );
    });
}

/// Objects dropped from the bundle cache — to stay inside its byte budget, or
/// because the entry cap bound.
///
/// Takes a count rather than being called per victim: one insert can evict
/// thousands (a large object must free its own size against small chunks), and
/// the caller holds a process-global lock on the custom-app asset path while it
/// does. Both eviction causes are counted, so "zero evictions" cannot read as
/// "the budget is generous" on a cache evicting steadily on entries.
///
/// Unlabelled: the cache is process-global and shared across every app, so
/// there is no org to attribute an eviction to — the victim and the cause are
/// usually different tenants, which is the whole reason the budget exists.
pub fn bundle_cache_evictions(count: u64) {
    with_instruments(|i| i.custom_app_bundle_cache_evictions.add(count, &[]));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recording must be safe with no provider installed — most of the test
    /// suite, every CLI subcommand, and any deployment with
    /// `OTEL_SDK_DISABLED` run in exactly that state.
    #[test]
    fn recording_without_a_provider_is_a_no_op_not_a_panic() {
        custom_app_request("org", "app", "html", 200, 0.1);
        custom_app_function("org", "app", "fn", "ok", 0.5, Some(0.006), 3);
        custom_app_function("org", "app", "fn", "error", 0.5, None, 0);
        custom_app_heap_termination("org");
        custom_app_admission_wait("org", 0.0);
        custom_app_admission_shed("org", "global");
    }
}
