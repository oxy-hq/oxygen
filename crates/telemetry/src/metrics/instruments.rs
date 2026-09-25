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
//! 2. **Custom-app metrics label every app and function by default, inside a
//!    fixed per-process budget.** At 44 apps across 6 orgs labelling all of
//!    them is cheap; the budget is what keeps the app and function axes from
//!    growing with the estate. There is nothing to configure. See
//!    [`app_label`]. It is not the only bound: the OpenTelemetry SDK caps every
//!    instrument at 2,000 attribute sets and folds the rest into one
//!    `otel.metric.overflow` series, which carries no org. The budget sits well
//!    inside that, so the SDK's overflow is a backstop that should never fire —
//!    and if it does, attribution is lost, not just detail.

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::sync::RwLock;
use std::sync::atomic::Ordering;

use opentelemetry::metrics::{
    Counter, Histogram, Meter, ObservableCounter, ObservableGauge, UpDownCounter,
};

use super::sources;

/// The label a custom-app series carries in place of an app or function
/// identifier once this process has spent its label budget. Deliberately not
/// empty: an empty label value is indistinguishable from an absent one in
/// Prometheus, and "we folded this" should be visible rather than inferred.
///
/// In practice it should never appear. If it does, the estate has outgrown
/// [`MAX_LABELLED_APPS`] or [`MAX_LABELLED_FUNCTIONS`] within one process
/// lifetime, and that is the signal to revisit them.
pub const APP_LABEL_OTHER: &str = "__other__";

/// The most distinct apps one process labels by id; the rest fold to
/// [`APP_LABEL_OTHER`].
///
/// A built-in default rather than a setting, deliberately. This replaced an
/// `OXY_METRICS_APP_WATCHLIST` env var that had to hold a hand-maintained list
/// of app UUIDs per environment — the kind of knob that makes operations hard
/// and was unset everywhere, so in practice every app folded and the per-app
/// and per-function panels were permanently empty.
///
/// First-come: an app is admitted the first time it records, and a process
/// restarts on every deploy, so what the budget has to cover is the apps that
/// see traffic within one process lifetime. Sized from prod, 2026-09-24: 36
/// apps had any traffic over 7 days and 45 over 14, so 64 leaves headroom for
/// a long-lived pod without letting the axis scale with the estate. Ids, not
/// slugs, because that is what the recording sites hold. The worst case the two
/// budgets allow is pinned by `the_label_budget_bounds_series_per_process`.
pub const MAX_LABELLED_APPS: usize = 64;

/// The most distinct `(app, function)` pairs one process labels by function
/// name; past it the function axis folds to [`APP_LABEL_OTHER`] while the app
/// keeps its id.
///
/// Budgeted per pair, not per name: two apps each exporting `handler` are two
/// sets of series. Function names are author-chosen, which is why this axis —
/// not the app axis — is the one that could otherwise run away. Sized from
/// prod, 2026-09-24: 95 distinct pairs ran over 7 days, 60 of them in the two
/// store-ops apps alone.
pub const MAX_LABELLED_FUNCTIONS: usize = 128;

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

/// Seconds buckets for isolate setup. Measured 2026-09-24 in dev and prod:
/// p50 ~40 ms, p95 ~95 ms, none under 10 ms, and 7-9% of invocation wall time.
/// (An earlier "mean 6 ms" here was wrong by several times.) Those are
/// worker-run isolates only — route-mode runs on the ide, which is not scraped —
/// so treat the ide's init as unmeasured. This is the series that decides the
/// startup-snapshot work: init's share of wall time is the ceiling on what a
/// snapshot could save.
///
/// Ranged over that distribution: four edges across the 25-100 ms band where
/// the mass sits, so `histogram_quantile` resolves p50 and p95 to within ~10 ms
/// instead of a factor of two. The previous set spent four of its ten edges
/// below 10 ms, where nothing has ever landed.
const INIT_DURATION_BUCKETS: &[f64] = &[
    0.005, 0.01, 0.02, 0.03, 0.04, 0.05, 0.075, 0.1, 0.15, 0.25, 0.5, 1.0,
];

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

    /// `oxy.custom_app.bundle_cache.evictions` — objects dropped from the
    /// bundle cache to stay inside its resident-byte budget.
    ///
    /// Zero means the budget never binds and the cache is sized generously.
    /// A high rate against a low hit rate means it is thrashing — the budget
    /// is too small for the working set, and raising it is cheaper than the
    /// store round-trips it is costing.
    pub custom_app_bundle_cache_evictions: Counter<u64>,

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
            custom_app_bundle_cache_evictions: meter
                .u64_counter("oxy.custom_app.bundle_cache.evictions")
                .with_description(
                    "Objects dropped from the bundle cache to stay inside its resident-byte \
                     budget. Zero means the budget never binds; a high rate means it is thrashing \
                     and is costing store round-trips.",
                )
                .with_unit("{object}")
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
                     there is no threshold to tune. Counted by whichever process ran the \
                     isolate: route-mode invocations run on the ide (the /fn/ route is \
                     IdeOnly), and task-queued ones (schedules, webhooks, manual and \
                     Airway-step runs) on whichever of the worker or the ide claims them — \
                     the ide runs in-process task drivers too. A role whose /metrics is not \
                     scraped contributes nothing, so an absent series is not a healthy zero.",
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
        ObservableHandle::U64(
            meter
                .u64_observable_gauge("oxy.custom_app.bundle_cache.bytes")
                .with_description(
                    "Bytes the custom-app bundle cache holds right now. Its ceiling used to be a \
                     per-entry COUNT, so the resident size was set by what tenants publish rather \
                     than by anything we chose; this is the measured half of the byte budget that \
                     replaced it.",
                )
                .with_unit("By")
                .with_callback(|observer| {
                    observer.observe(sources::BUNDLE_CACHE_BYTES.load(Ordering::Relaxed), &[]);
                })
                .build(),
        ),
        ObservableHandle::U64(
            meter
                .u64_observable_gauge("oxy.custom_app.bundle_cache.limit")
                .with_description(
                    "The bundle cache's configured resident-byte budget; 0 when the byte bound is \
                     disabled. Published when the budget is resolved, so 0 means genuinely off \
                     rather than nothing cached yet.",
                )
                .with_unit("By")
                .with_callback(|observer| {
                    observer.observe(sources::BUNDLE_CACHE_LIMIT.load(Ordering::Relaxed), &[]);
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

/// A first-come, fixed-size set of label values.
///
/// Values already admitted are always admitted again, so a label never flips
/// between its own value and [`APP_LABEL_OTHER`] within one process; only a
/// value first seen after the budget is spent folds.
struct LabelBudget {
    cap: usize,
    admitted: RwLock<BTreeSet<String>>,
}

impl LabelBudget {
    const fn new(cap: usize) -> Self {
        Self {
            cap,
            admitted: RwLock::new(BTreeSet::new()),
        }
    }

    fn admit(&self, value: &str) -> bool {
        // A poisoned lock only means another thread panicked mid-insert; the
        // set is still a valid set, and a metric label is no reason to spread
        // that panic into a request path.
        {
            let admitted = self.admitted.read().unwrap_or_else(|e| e.into_inner());
            if admitted.contains(value) {
                return true;
            }
            // Full: answer from the shared lock. Without this, every request
            // for a folded app would take the exclusive lock just to be told
            // no — and a waiting writer blocks new readers, so a folded app's
            // asset burst would stall the admitted apps' requests too.
            if admitted.len() >= self.cap {
                return false;
            }
        }
        let mut admitted = self.admitted.write().unwrap_or_else(|e| e.into_inner());
        if admitted.contains(value) {
            return true; // admitted by another thread between the two locks
        }
        if admitted.len() >= self.cap {
            return false;
        }
        admitted.insert(value.to_owned());
        true
    }
}

static LABELLED_APPS: LabelBudget = LabelBudget::new(MAX_LABELLED_APPS);
static LABELLED_FUNCTIONS: LabelBudget = LabelBudget::new(MAX_LABELLED_FUNCTIONS);

/// The value a custom-app series carries for the app axis: the app's own
/// **id**, until this process has labelled [`MAX_LABELLED_APPS`] distinct apps,
/// and [`APP_LABEL_OTHER`] for any app first seen after that.
///
/// Within one process, serve-plane requests and function invocations share the
/// one budget. That is a per-process property only: asset requests record on
/// the serve fleet, route-mode invocations on the ide, and task-queued ones on
/// whichever of the worker or the ide claimed them — each with its own budget. Below the cap — the normal state —
/// every process labels every app, so the distinction only shows under
/// overflow, when an app can be labelled on one pod and folded on another.
pub fn app_label(app_id: &str) -> Cow<'static, str> {
    if LABELLED_APPS.admit(app_id) {
        Cow::Owned(app_id.to_owned())
    } else {
        Cow::Borrowed(APP_LABEL_OTHER)
    }
}

/// The value a function series carries for the function axis, for an app that
/// is itself labelled: the function's name, until this process has labelled
/// [`MAX_LABELLED_FUNCTIONS`] distinct `(app, function)` pairs, and
/// [`APP_LABEL_OTHER`] after that.
pub fn function_label(app_id: &str, function: &str) -> Cow<'static, str> {
    if LABELLED_FUNCTIONS.admit(&function_key(app_id, function)) {
        Cow::Owned(function.to_owned())
    } else {
        Cow::Borrowed(APP_LABEL_OTHER)
    }
}

/// The budget key for one `(app, function)` pair. The separator is a control
/// character no app id or function name contains, so two different pairs can
/// never produce the same key.
fn function_key(app_id: &str, function: &str) -> String {
    format!("{app_id}\u{1f}{function}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_label_budget_admits_up_to_its_cap_and_never_evicts() {
        let budget = LabelBudget::new(2);
        assert!(budget.admit("a"));
        assert!(budget.admit("b"));
        assert!(!budget.admit("c"), "past the cap a NEW value must fold");
        // An admitted value stays admitted, so its series never flips between
        // its own label and __other__ inside one process lifetime.
        assert!(budget.admit("a"));
        assert!(budget.admit("b"));
        assert!(!budget.admit("c"), "and a folded value stays folded");
    }

    /// The whole point of replacing the watchlist: an app is labelled by its id
    /// with nothing configured. The watchlist's default was "fold everything",
    /// which left every per-app panel empty in every environment.
    #[test]
    fn apps_and_functions_are_labelled_by_default() {
        let app = "0f8fad5b-d9cb-469f-a165-70867728950e";
        assert_eq!(app_label(app), app);
        assert_eq!(function_label(app, "handler"), "handler");
    }

    #[test]
    fn the_function_budget_is_per_app_function_pair() {
        // Two apps that both export `handler` are two sets of series, so they
        // must cost two slots — and must not collide into one key.
        assert_ne!(
            function_key("app-a", "handler"),
            function_key("app-b", "handler")
        );
        assert_ne!(function_key("ab", "c"), function_key("a", "bc"));
    }

    /// The worst case the budgets allow, per process, must stay bounded — this
    /// is what the watchlist used to buy by folding everything. Raising a cap
    /// past this ceiling should be a decision, so it fails here first.
    #[test]
    fn the_label_budget_bounds_series_per_process() {
        // `success | error | timeout | cancelled | shed` — every status a run
        // records in custom_apps_functions (`shed` is an unadmitted run).
        const FUNCTION_OUTCOMES: usize = 5;
        // An allowance, not a count: prod showed ~4 statuses per (app, kind)
        // over 14 days.
        const REQUEST_STATUSES: usize = 8;
        // Org is on every series and no budget bounds it. It does not multiply
        // a labelled app (an app has one org), but a FOLDED app becomes
        // `(org, __other__)`, so the fold bucket fans out per org. 6 today.
        const ORGS: usize = 16;

        // A histogram exposes its buckets plus +Inf, _sum and _count.
        let hist = |b: &[f64]| b.len() + 3;
        let per_function_slot = FUNCTION_OUTCOMES
            * (1 // the invocations counter
                + hist(FUNCTION_DURATION_BUCKETS)
                + hist(INIT_DURATION_BUCKETS)
                + hist(HOST_CALL_BUCKETS));
        // Function slots: every labelled pair; plus, once the function budget
        // is spent, one `oxy_function="__other__"` slot per labelled app; plus
        // one function-less slot per org for apps that folded.
        let function_series =
            per_function_slot * (MAX_LABELLED_FUNCTIONS + MAX_LABELLED_APPS + ORGS);
        // Request slots: kind is html|asset, per labelled app and per org fold.
        let request_series =
            2 * REQUEST_STATUSES * hist(HTTP_DURATION_BUCKETS) * (MAX_LABELLED_APPS + ORGS);

        // 220 x (128 + 64 + 16) + 272 x (64 + 16) = 45,760 + 21,760 = 67,520.
        // A ceiling, not an expectation: a real week of prod is about 4k. The
        // SDK's own 2,000-attribute-set cap per instrument is the absolute
        // backstop above this (~120k across these five instruments).
        let worst = function_series + request_series;
        assert!(
            worst <= 70_000,
            "the label budgets allow {worst} custom-app series per process \
             ({function_series} function + {request_series} request) — past the \
             70k ceiling. That is per pod, across every pod that serves /metrics."
        );
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
