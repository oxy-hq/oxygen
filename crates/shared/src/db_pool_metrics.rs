//! The Postgres pool's metrics, as data both of their halves read: the pool
//! health monitor that publishes them (`oxy-platform`'s `db::client`) and the
//! metrics runtime that exports them (`oxy-telemetry`'s `metrics`).
//!
//! Here, in the kernel, so the publisher need not depend on the exporter
//! (`internal-docs/crate-tiers.md`). Two kinds of thing:
//!
//! - **The gauges' values**: plain atomics the monitor `store`s and
//!   telemetry's observable callbacks `load` (moved from
//!   `oxy_telemetry::metrics::sources`, which re-exports them).
//! - **The two sync instruments' identity**: name, description, unit, buckets
//!   and the `oxy.reason` label set of `oxy.db.pool.probe.duration` and
//!   `oxy.db.pool.probe.failures`. The monitor records them through the
//!   OpenTelemetry API's global meter; telemetry builds the same instruments to
//!   seed the zero series. One definition, so the two always resolve to the
//!   same stream.
//!
//! Every value is per-process: a fleet of N replicas reports N series.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

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

/// How many times the pool has *entered* starvation — episodes, not probes.
///
/// A pool that stays starved across ten consecutive probes counts **one**;
/// [`set_db_pool_starved`] increments only on the false-to-true edge, and
/// `starvation_events_count_transitions_not_probes` below pins that. Counting
/// probes instead would make the number track probe cadence rather than how
/// often the pool broke.
///
/// Monotonic and per-process. The gauge above says "right now"; this says "has
/// it been happening", which is what distinguishes a blip from a pattern
/// between two scrapes. For failure *volume* and the split by cause,
/// read `oxy_db_pool_probe_failures_total`, which counts every failed probe —
/// the two answer different questions and neither substitutes for the other.
pub static DB_POOL_STARVATION_EVENTS: AtomicU64 = AtomicU64::new(0);

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

/// The meter scope every Oxy instrument is created under (telemetry's
/// `SCOPE`, which a test there pins to this).
pub const METER_SCOPE: &str = "oxy";

/// `oxy.db.pool.probe.duration`: how long one successful probe waited.
pub const PROBE_DURATION_NAME: &str = "oxy.db.pool.probe.duration";
pub const PROBE_DURATION_DESCRIPTION: &str = "Time the pool health probe waited for a connection. \
     A sample of contention, not the latency real traffic sees.";
pub const PROBE_DURATION_UNIT: &str = "s";

/// Seconds buckets for the pool health probe's checkout.
///
/// The probe is cancelled at `POOL_HEALTH_PROBE_TIMEOUT` (2 s), **not** at the
/// 30 s `ACQUIRE_TIMEOUT` that real traffic waits out, so the whole observable
/// range ends at 2 s. A probe that times out records no duration at all — it
/// increments the failure counter instead, which is why the two instruments
/// have to be read together.
pub const PROBE_DURATION_BUCKETS: &[f64] =
    &[0.0005, 0.001, 0.005, 0.01, 0.05, 0.1, 0.25, 0.5, 1.0, 2.0];

/// `oxy.db.pool.probe.failures`: probes that did not acquire, by `oxy.reason`.
pub const PROBE_FAILURES_NAME: &str = "oxy.db.pool.probe.failures";
pub const PROBE_FAILURES_DESCRIPTION: &str = "Pool health probes that did not acquire, by reason: \
     timeout (pool exhausted), server_unavailable (pool had room; waited on Postgres), error \
     (checkout failed). These record no duration, so this is not the histogram's _count.";
pub const PROBE_FAILURES_UNIT: &str = "{probe}";

/// The failure counter's one label key.
pub const PROBE_FAILURE_REASON_KEY: &str = "oxy.reason";

/// The `reason` values a failed probe can carry.
///
/// One definition for the probe that selects them (`oxy-platform`) and the
/// seed that zeroes them (`oxy-telemetry`'s `seed_zero_series`): two string
/// literals in two crates drift without a compile error, which would leave a
/// seeded decoy at 0 beside the real series.
///
/// `timeout` means the pool was **exhausted**: the probe's 2 s ran out with
/// every connection the pool may open already open.
pub const DB_POOL_PROBE_FAILURE_TIMEOUT: &str = "timeout";
/// The probe's 2 s ran out while the pool still had room to open a connection,
/// so the wait was on **Postgres**, not on the pool: it is down, restarting,
/// refusing (`too many clients`), or too slow to accept.
///
/// This is its own value because sqlx hides the difference. It retries a
/// refused connect, `53300` and `57P03` inside `acquire()` until its own 30 s
/// deadline, so the probe's shorter timeout always wins and a server outage
/// used to be labelled `timeout`, exactly like a full pool — the opposite fix.
/// Raising the pool ceiling during a server-side shortage makes it worse.
pub const DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE: &str = "server_unavailable";
/// `acquire()` returned an error before the timeout: a failure sqlx does not
/// retry, such as bad credentials, TLS, DNS or a closed pool. Configuration,
/// not capacity.
pub const DB_POOL_PROBE_FAILURE_ERROR: &str = "error";

/// Every `reason` the probe may record, and so every one seeded at zero.
pub const DB_POOL_PROBE_FAILURE_REASONS: [&str; 3] = [
    DB_POOL_PROBE_FAILURE_TIMEOUT,
    DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE,
    DB_POOL_PROBE_FAILURE_ERROR,
];

#[cfg(test)]
mod tests {
    use super::*;

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
