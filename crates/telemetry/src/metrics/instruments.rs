//! Every instrument Oxy records, defined once.
//!
//! Naming follows two rules, and the split is deliberate:
//!
//! - **HTTP metrics use the OpenTelemetry semantic conventions verbatim**
//!   (`http.server.request.duration`). They are the one set where off-the-shelf
//!   dashboards and the collector's own processors already know the names, and
//!   inventing `oxy_http_*` would forfeit that for nothing.
//! - **Everything Oxy-specific is `oxy.*`** — pool occupancy, custom-app serve
//!   and function timings, isolate saturation. These describe our own
//!   mechanisms and have no convention to follow.
//!
//! Prometheus renders both through the standard translation (see
//! [`super::exposition`]): dots become underscores, a `s` unit appends
//! `_seconds`, and a monotonic sum appends `_total`.
//!
//! ## The attribute budget is the design constraint
//!
//! Every attribute multiplies series count, and the series live in
//! VictoriaMetrics alongside 252 existing rules. Two rules keep it bounded:
//!
//! 1. **Route labels come from `MatchedPath`**, never a raw URL path — the
//!    route pattern is already low-cardinality and already redacted for
//!    secret-shaped parameters. `crate::http_trace` owns that logic and this
//!    module reuses its output rather than re-deriving it, so a token can never
//!    reach a label.
//! 2. **Custom-app metrics are labelled by org, not by app, unless the app is
//!    on an explicit watchlist.** At 44 apps across 6 orgs the difference is
//!    small; the point is that it stays small when there are 4,400. See
//!    [`app_label`].

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use opentelemetry::metrics::{
    Counter, Histogram, Meter, ObservableCounter, ObservableGauge, UpDownCounter,
};

use super::sources;

/// The label a custom-app series carries in place of an app identifier when the
/// app is not on the watchlist. Deliberately not empty: an empty label value is
/// indistinguishable from an absent one in Prometheus, and "we folded this"
/// should be visible rather than inferred.
pub const APP_LABEL_OTHER: &str = "__other__";

/// Comma-separated app **ids** — UUIDs, not slugs — whose metrics carry a
/// per-app label.
///
/// Ids rather than slugs because that is what the recording sites have:
/// `custom_apps_telemetry` holds `app_id: Uuid` and never resolves a slug on
/// the hot path. Configuring this with slugs is not an error, it simply never
/// matches, so the wording here has to be unambiguous.
pub const APP_WATCHLIST_ENV: &str = "OXY_METRICS_APP_WATCHLIST";

/// Seconds buckets for inbound HTTP. The semconv-recommended set for
/// `http.server.request.duration`, unchanged — a shared boundary set is what
/// lets one recording rule cover both this and any other semconv-shaped source.
const HTTP_DURATION_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// Seconds buckets for a function invocation. Measured p95 is 637 ms and the
/// default wall timeout is 10 s with a 300 s ceiling, so the useful resolution
/// is sub-second with a long tail that must still separate "slow" from "hit the
/// timeout" — a bucket edge sits exactly on 10 s for that reason.
const FUNCTION_DURATION_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
];

/// Seconds buckets for isolate setup. Mean init is 6 ms, so the whole
/// interesting range is below what [`FUNCTION_DURATION_BUCKETS`] can resolve.
/// This is the series that would justify (or, more likely, keep deferring) the
/// startup-snapshot work.
const INIT_DURATION_BUCKETS: &[f64] =
    &[0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0];

/// Host calls (warehouse, fetch, storage, …) per invocation. Separates "one
/// slow dependency" from "a function looping queries", which a duration alone
/// cannot.
const HOST_CALL_BUCKETS: &[f64] = &[1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0];

/// Seconds buckets for the pool health probe's checkout.
///
/// The probe is cancelled at `POOL_HEALTH_PROBE_TIMEOUT` (2 s), **not** at the
/// 30 s `ACQUIRE_TIMEOUT` that real traffic waits out, so the whole observable
/// range ends at 2 s. A probe that times out records no duration at all — it
/// increments the failure counter instead, which is why the two instruments
/// have to be read together.
const POOL_PROBE_BUCKETS: &[f64] = &[0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0];

/// Seconds buckets for the admission queue.
///
/// Healthy is the bottom bucket — an uncontended permit is acquired in
/// microseconds, so the resolution that matters is "did it wait at all". The
/// top edge sits on the default queue budget so a wait that ended in a shed is
/// distinguishable from one that merely came close.
const ADMISSION_WAIT_BUCKETS: &[f64] = &[0.0001, 0.001, 0.01, 0.05, 0.1, 0.5, 1.0, 2.0, 5.0];

/// The instrument set, built once and reached through [`super::instruments`].
///
/// Observable instruments are held here rather than dropped after
/// registration: the handle owns the callback registration, and dropping it
/// would silently stop the series.
pub struct Instruments {
    /// `http.server.request.duration` — the RED triad for every inbound
    /// request. Rate and errors are both derivable from this one histogram
    /// (`_count` and a `http.response.status_code` filter), so there is no
    /// separate request counter.
    pub http_request_duration: Histogram<f64>,
    /// `http.server.active_requests` — in-flight requests. The saturation
    /// signal a duration histogram cannot give: a queue builds here before it
    /// shows up as latency.
    pub http_active_requests: UpDownCounter<i64>,

    /// `oxy.db.pool.probe.duration` — how long the pool health probe waited
    /// for a connection.
    ///
    /// **This times the probe, not every checkout.** Instrumenting real traffic
    /// would mean wrapping every `acquire()` behind Sea-ORM, which this does
    /// not do; the probe is an existing 30 s-cadence sample of the same
    /// contention. Named `probe` rather than `acquire` so nobody reads a p99
    /// here as the p99 a request experiences.
    ///
    /// It is still the signal that would have named the prod incident where
    /// `max_connections` sat pending-reboot and the API stalled 30 s at a time:
    /// sqlx swallows the Postgres `FATAL`, so the only in-band evidence was
    /// latency that looked like slow queries.
    pub db_pool_probe_duration: Histogram<f64>,
    /// `oxy.db.pool.probe.failures` — probes that did not acquire, by
    /// `oxy.reason`.
    ///
    /// Two shapes, two incidents. **`timeout`** is the pool being full —
    /// nothing freed within the probe's 2 s budget. **`error`** is the *server*
    /// refusing us, which is what the `max_connections` pending-reboot stall
    /// looked like from in here.
    ///
    /// Neither records a duration: a timeout never finished, and an error
    /// finished without acquiring, so its elapsed time measures how fast the
    /// server said no rather than how long a checkout takes. So this counter is
    /// not redundant with the histogram's `_count` — the difference between
    /// them is exactly the probes that did not acquire.
    pub db_pool_probe_failures: Counter<u64>,

    /// `oxy.custom_app.request.duration` — the serve plane, by org and kind.
    pub custom_app_request_duration: Histogram<f64>,
    /// `oxy.custom_app.function.duration` — wall time of one invocation.
    pub custom_app_function_duration: Histogram<f64>,
    /// `oxy.custom_app.function.init.duration` — the setup half of the above.
    /// `duration - init` is the tenant's own code.
    pub custom_app_function_init_duration: Histogram<f64>,
    /// `oxy.custom_app.function.host_calls` — subrequests per invocation.
    pub custom_app_function_host_calls: Histogram<f64>,
    /// `oxy.custom_app.function.invocations` — count by outcome. Redundant with
    /// the duration histogram's `_count` for successes, but an invocation that
    /// fails before it is timed still has to be counted somewhere.
    pub custom_app_function_invocations: Counter<u64>,

    /// `oxy.custom_app.isolates.heap_terminations` — isolates killed for
    /// breaching their heap ceiling.
    ///
    /// Deliberately its own series rather than an `outcome` label on the
    /// invocation counter: the invocation row records this as a plain `error`
    /// (adding a new status string would be a wire change for every consumer
    /// of `app_function_invocations`), so this counter is the *only* place the
    /// distinction survives. A non-zero value means a tenant is writing code
    /// that would previously have OOM-killed the whole serve process.
    pub custom_app_heap_terminations: Counter<u64>,

    /// `oxy.custom_app.admission.wait` — how long an invocation queued for a
    /// concurrency permit before running.
    ///
    /// Near-zero at healthy load. This rising is the early warning that the cap
    /// is binding, and it rises well before anything is shed.
    pub custom_app_admission_wait: Histogram<f64>,
    /// `oxy.custom_app.admission.shed` — invocations rejected because no permit
    /// came free within the queue budget, by `oxy.reason` (`global` / `org`).
    pub custom_app_admission_shed: Counter<u64>,

    /// Observable handles. Never read; held so their callbacks stay registered.
    _observables: Vec<ObservableHandle>,
}

/// Type-erased storage for the observable handles, which have different
/// generic parameters but the same job here: exist.
enum ObservableHandle {
    U64(#[allow(dead_code)] ObservableGauge<u64>),
    I64(#[allow(dead_code)] ObservableGauge<i64>),
    Counter(#[allow(dead_code)] ObservableCounter<u64>),
}

impl std::fmt::Debug for Instruments {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Instruments")
    }
}

impl Instruments {
    /// Build every instrument against `meter`, registering the observable
    /// callbacks that read [`super::sources`].
    pub fn new(meter: &Meter) -> Self {
        Self {
            http_request_duration: meter
                .f64_histogram("http.server.request.duration")
                .with_description("Duration of inbound HTTP server requests.")
                .with_unit("s")
                .with_boundaries(HTTP_DURATION_BUCKETS.to_vec())
                .build(),
            http_active_requests: meter
                .i64_up_down_counter("http.server.active_requests")
                .with_description("Inbound HTTP requests currently being served.")
                .with_unit("{request}")
                .build(),

            db_pool_probe_duration: meter
                .f64_histogram("oxy.db.pool.probe.duration")
                .with_description(
                    "Time the pool health probe waited for a connection. A sample of contention, \
                     not the latency real traffic sees.",
                )
                .with_unit("s")
                .with_boundaries(POOL_PROBE_BUCKETS.to_vec())
                .build(),
            db_pool_probe_failures: meter
                .u64_counter("oxy.db.pool.probe.failures")
                .with_description(
                    "Pool health probes that timed out. These record no duration, so this is not \
                     the histogram's _count.",
                )
                .with_unit("{probe}")
                .build(),

            custom_app_request_duration: meter
                .f64_histogram("oxy.custom_app.request.duration")
                .with_description("Duration of a custom-app serve-plane request, by org and kind.")
                .with_unit("s")
                .with_boundaries(HTTP_DURATION_BUCKETS.to_vec())
                .build(),
            custom_app_function_duration: meter
                .f64_histogram("oxy.custom_app.function.duration")
                .with_description("Wall time of one Oxy Function invocation.")
                .with_unit("s")
                .with_boundaries(FUNCTION_DURATION_BUCKETS.to_vec())
                .build(),
            custom_app_function_init_duration: meter
                .f64_histogram("oxy.custom_app.function.init.duration")
                .with_description(
                    "Setup time before tenant code runs; duration minus this is the tenant's own.",
                )
                .with_unit("s")
                .with_boundaries(INIT_DURATION_BUCKETS.to_vec())
                .build(),
            custom_app_function_host_calls: meter
                .f64_histogram("oxy.custom_app.function.host_calls")
                .with_description("Host calls (warehouse, fetch, storage) made by one invocation.")
                .with_unit("{call}")
                .with_boundaries(HOST_CALL_BUCKETS.to_vec())
                .build(),
            custom_app_function_invocations: meter
                .u64_counter("oxy.custom_app.function.invocations")
                .with_description("Oxy Function invocations by outcome.")
                .with_unit("{invocation}")
                .build(),

            custom_app_heap_terminations: meter
                .u64_counter("oxy.custom_app.isolates.heap_terminations")
                .with_description(
                    "Isolates terminated for breaching their per-isolate heap ceiling. Before the \
                     ceiling existed this was a process-wide OOM kill; a non-zero value here is a \
                     tenant that would previously have taken the serve fleet down.",
                )
                .with_unit("{isolate}")
                .build(),
            custom_app_admission_wait: meter
                .f64_histogram("oxy.custom_app.admission.wait")
                .with_description(
                    "Time an invocation queued for a concurrency permit. Rises well before \
                     anything is shed, so it is the early warning that the cap is binding.",
                )
                .with_unit("s")
                .with_boundaries(ADMISSION_WAIT_BUCKETS.to_vec())
                .build(),
            custom_app_admission_shed: meter
                .u64_counter("oxy.custom_app.admission.shed")
                .with_description(
                    "Invocations rejected because no permit came free within the queue budget, by \
                     whether the global or the per-org limit bound.",
                )
                .with_unit("{invocation}")
                .build(),

            _observables: observables(meter),
        }
    }
}

/// Register the pull-based gauges. Each reads an atomic from
/// [`super::sources`]; see that module for why the seam is atomics and not a
/// callback registry.
fn observables(meter: &Meter) -> Vec<ObservableHandle> {
    vec![
        ObservableHandle::U64(
            meter
                .u64_observable_gauge("oxy.db.pool.connections")
                .with_description(
                    "Connections this process's Postgres pool holds, by state. Per-process: \
                     sum across replicas for the fleet total, never max.",
                )
                .with_unit("{connection}")
                .with_callback(|observer| {
                    let size = sources::DB_POOL_SIZE.load(Ordering::Relaxed);
                    let idle = sources::DB_POOL_IDLE.load(Ordering::Relaxed);
                    observer.observe(idle, &[kv("state", "idle")]);
                    observer.observe(size.saturating_sub(idle), &[kv("state", "in_use")]);
                })
                .build(),
        ),
        ObservableHandle::U64(
            meter
                .u64_observable_gauge("oxy.db.pool.max")
                .with_description(
                    "This process's pool ceiling. The denominator of the saturation ratio, \
                     exported so the ratio needs no out-of-band constant.",
                )
                .with_unit("{connection}")
                .with_callback(|observer| {
                    observer.observe(sources::DB_POOL_MAX.load(Ordering::Relaxed), &[]);
                })
                .build(),
        ),
        ObservableHandle::U64(
            meter
                .u64_observable_gauge("oxy.db.pool.starved")
                .with_description(
                    "1 while this process's pool health probe cannot acquire a connection. \
                     Aggregate with max: one starved replica is an incident.",
                )
                .with_unit("1")
                .with_callback(|observer| {
                    let starved = u64::from(sources::DB_POOL_STARVED.load(Ordering::Relaxed));
                    observer.observe(starved, &[]);
                })
                .build(),
        ),
        // An observable COUNTER, not a gauge. The value is monotonic and the
        // whole point of it is `increase()` over a window — which Prometheus
        // does not accept on a gauge, and which needs the `_total` suffix that
        // only a monotonic sum gets from `exposition`. Registered as a gauge
        // this rendered `# TYPE oxy_db_pool_starvation_events gauge`, so an
        // operator following the doc would have written a rule against a
        // `..._total` series that does not exist — an alert that can never
        // fire, which is the exact failure class this module exists to close.
        ObservableHandle::Counter(
            meter
                .u64_observable_counter("oxy.db.pool.starvation_events")
                .with_description(
                    "Transitions into pool starvation since process start. Monotonic; catches a \
                     starvation that opened and closed between two scrapes, which the gauge \
                     cannot.",
                )
                .with_unit("{event}")
                .with_callback(|observer| {
                    observer.observe(
                        sources::DB_POOL_STARVATION_EVENTS.load(Ordering::Relaxed),
                        &[],
                    );
                })
                .build(),
        ),
        ObservableHandle::I64(
            meter
                .i64_observable_gauge("oxy.custom_app.isolates.live")
                .with_description(
                    "V8 isolates alive in this process. One per in-flight invocation, each on \
                     its own OS thread outside Tokio's pool. Nothing bounds this today.",
                )
                .with_unit("{isolate}")
                .with_callback(|observer| {
                    observer.observe(sources::ISOLATES_LIVE.load(Ordering::Relaxed), &[]);
                })
                .build(),
        ),
        ObservableHandle::Counter(
            meter
                .u64_observable_counter("oxy.custom_app.isolates.abandoned")
                .with_description(
                    "Isolate threads detached after the termination grace period — a tenant \
                     function wedged in a host call that never returned. Healthy is zero, so \
                     there is no threshold to tune. Emitted by every role, which is the fix for \
                     it previously being exported only by the worker, the one fleet that never \
                     creates an isolate.",
                )
                .with_unit("{isolate}")
                .with_callback(|observer| {
                    observer.observe(sources::ISOLATES_ABANDONED.load(Ordering::Relaxed), &[]);
                })
                .build(),
        ),
        ObservableHandle::I64(
            meter
                .i64_observable_gauge("oxy.custom_app.admission.in_use")
                .with_description(
                    "Concurrency permits held by running invocations. Compare with \
                     oxy.custom_app.isolates.live: the gap is isolates that were abandoned and so \
                     hold a thread and a heap without holding a permit — the cap no longer \
                     accounts for them.",
                )
                .with_unit("{permit}")
                .with_callback(|observer| {
                    observer.observe(sources::ADMISSION_IN_USE.load(Ordering::Relaxed), &[]);
                })
                .build(),
        ),
        ObservableHandle::I64(
            meter
                .i64_observable_gauge("oxy.custom_app.admission.queued")
                .with_description(
                    "Invocations parked waiting for a concurrency permit. Not free: each waiter \
                     retains its function bundle, context and host for up to the queue budget, \
                     so a deep queue consumes the same memory headroom the concurrency cap is \
                     spending — while in_use and isolates.live both read healthy.",
                )
                .with_unit("{invocation}")
                .with_callback(|observer| {
                    observer.observe(sources::ADMISSION_QUEUED.load(Ordering::Relaxed), &[]);
                })
                .build(),
        ),
        ObservableHandle::I64(
            meter
                .i64_observable_gauge("oxy.custom_app.admission.limit")
                .with_description(
                    "The configured global permit ceiling; 0 when the cap is disabled. The \
                     denominator of the saturation ratio, exported so it needs no out-of-band \
                     constant.",
                )
                .with_unit("{permit}")
                .with_callback(|observer| {
                    observer.observe(sources::ADMISSION_LIMIT.load(Ordering::Relaxed), &[]);
                })
                .build(),
        ),
        ObservableHandle::I64(
            meter
                .i64_observable_gauge("oxy.custom_app.isolates.live_peak")
                .with_description(
                    "High-water mark of live isolates since process start. The number to size a \
                     concurrency cap and a per-isolate heap ceiling against — a scrape-interval \
                     gauge misses the spike that matters.",
                )
                .with_unit("{isolate}")
                .with_callback(|observer| {
                    observer.observe(sources::ISOLATES_LIVE_PEAK.load(Ordering::Relaxed), &[]);
                })
                .build(),
        ),
    ]
}

fn kv(key: &'static str, value: &'static str) -> opentelemetry::KeyValue {
    opentelemetry::KeyValue::new(key, value)
}

/// The app watchlist, parsed once from [`APP_WATCHLIST_ENV`].
fn watchlist() -> &'static BTreeSet<String> {
    static WATCHLIST: std::sync::OnceLock<BTreeSet<String>> = std::sync::OnceLock::new();
    WATCHLIST.get_or_init(|| parse_watchlist(std::env::var(APP_WATCHLIST_ENV).ok().as_deref()))
}

fn parse_watchlist(raw: Option<&str>) -> BTreeSet<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The value a custom-app series carries for the app axis.
///
/// Returns the app's own **id** when it is on the watchlist (see
/// [`APP_WATCHLIST_ENV`] — ids, not slugs), and [`APP_LABEL_OTHER`] otherwise.
/// Org is always labelled, so folding here costs the ability to name *which*
/// app inside an org moved, not the ability to see that one did — and a
/// watchlist entry buys that back for the app under investigation without
/// paying for every app forever.
pub fn app_label(app_id: &str) -> Cow<'static, str> {
    if watchlist().contains(app_id) {
        Cow::Owned(app_id.to_owned())
    } else {
        Cow::Borrowed(APP_LABEL_OTHER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchlist_parsing_tolerates_spacing_and_empties() {
        // Ids, matching what the recording sites actually pass — the fixtures
        // used to read as slugs, which is how the rustdoc drifted.
        let set = parse_watchlist(Some(
            " 0f8fad5b-d9cb-469f-a165-70867728950e , ,7c9e6679-7425-40de-944b-e07fc1f90ae7,",
        ));
        assert_eq!(set.len(), 2);
        assert!(set.contains("0f8fad5b-d9cb-469f-a165-70867728950e"));
        assert!(set.contains("7c9e6679-7425-40de-944b-e07fc1f90ae7"));
    }

    #[test]
    fn an_unset_watchlist_is_empty_not_everything() {
        // The failure mode this guards: treating "no watchlist configured" as
        // "label every app", which is how a 44-series axis quietly becomes a
        // 4,400-series one.
        assert!(parse_watchlist(None).is_empty());
        assert!(parse_watchlist(Some("")).is_empty());
    }

    /// Bucket sets must be sorted and free of duplicates or the SDK rejects
    /// them and silently hands back a no-op instrument — a failure that looks
    /// exactly like "the metric was never recorded".
    #[test]
    fn every_bucket_set_is_strictly_ascending() {
        for (name, buckets) in [
            ("http", HTTP_DURATION_BUCKETS),
            ("function", FUNCTION_DURATION_BUCKETS),
            ("init", INIT_DURATION_BUCKETS),
            ("host_calls", HOST_CALL_BUCKETS),
            ("pool_probe", POOL_PROBE_BUCKETS),
            ("admission_wait", ADMISSION_WAIT_BUCKETS),
        ] {
            assert!(
                buckets.windows(2).all(|w| w[0] < w[1]),
                "{name} buckets are not strictly ascending: {buckets:?}"
            );
            assert!(!buckets.is_empty(), "{name} has no buckets");
        }
    }

    /// The 10 s edge is what separates a slow invocation from one that hit
    /// `ROUTE_DEFAULT_TIMEOUT_SECS`. Losing it would merge the two.
    #[test]
    fn function_buckets_have_an_edge_on_the_default_timeout() {
        assert!(
            FUNCTION_DURATION_BUCKETS.contains(&10.0),
            "the default wall timeout needs its own bucket edge"
        );
    }

    /// The probe is cancelled at 2s, so the top edge sits there. A wider range
    /// would be dead buckets; a narrower one would put every healthy probe in
    /// `+Inf` alongside the pathological ones.
    #[test]
    fn pool_probe_buckets_top_out_at_the_probe_timeout() {
        let last = *POOL_PROBE_BUCKETS.last().expect("non-empty");
        assert_eq!(
            last, 2.0,
            "the probe's own timeout is 2s (POOL_HEALTH_PROBE_TIMEOUT), not the \
             30s ACQUIRE_TIMEOUT real traffic waits out"
        );
    }
}
