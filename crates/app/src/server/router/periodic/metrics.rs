//! What the periodic driver's cloud tick did, for `GET /metrics`.
//!
//! Process-local atomics, read by `worker_metrics::metrics` through
//! [`push_prometheus`] — the same arrangement as the reap counters in
//! `agentic_runtime::crud`. They live here rather than in `worker_metrics`
//! because the tick is what increments them, and the tick must not depend on
//! the exposition module.
//!
//! All but one are per replica: the tick runs on every node that drives the
//! queue (every role but `serve`), and each node claims its own batch. So the
//! counters aggregate with `sum` across replicas — the sum of
//! `oxy_periodic_workspaces_claimed_total` over the fleet rises by the
//! workspace count once per driver interval when the fleet keeps up — and the
//! duration gauge is per replica, read with `max` for the slowest tick. The
//! exception is `oxy_periodic_due_backlog`, a count read from the shared
//! table: every replica reports (nearly) the same number, so take `max`, never
//! `sum`.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

/// Cloud ticks this process has run.
pub(crate) static TICKS: AtomicU64 = AtomicU64::new(0);
/// Workspaces this process claimed, over every tick.
pub(crate) static WORKSPACES_CLAIMED: AtomicU64 = AtomicU64::new(0);
/// Per-workspace schedule reconciles this process skipped because the
/// promoted revision had not changed since the last one.
pub(crate) static RECONCILES_SKIPPED_UNCHANGED: AtomicU64 = AtomicU64::new(0);
/// Wall time of the last tick, in milliseconds.
pub(crate) static LAST_TICK_MILLIS: AtomicU64 = AtomicU64::new(0);
/// Workspaces still due and unclaimed when the last tick yielded — after its
/// last claim, not before its first. DB-sourced.
pub(crate) static DUE_BACKLOG: AtomicI64 = AtomicI64::new(0);

/// Record one finished tick.
pub(super) fn record_tick(elapsed: Duration, claimed: usize) {
    TICKS.fetch_add(1, Ordering::Relaxed);
    WORKSPACES_CLAIMED.fetch_add(claimed as u64, Ordering::Relaxed);
    LAST_TICK_MILLIS.store(elapsed.as_millis() as u64, Ordering::Relaxed);
}

/// Record the due backlog read when a tick's drain yielded.
pub(super) fn record_backlog(due: i64) {
    DUE_BACKLOG.store(due, Ordering::Relaxed);
}

/// Record one reconcile the gate skipped.
pub(super) fn record_reconcile_skipped() {
    RECONCILES_SKIPPED_UNCHANGED.fetch_add(1, Ordering::Relaxed);
}

/// Append the tick's series in Prometheus text exposition.
///
/// Exported unconditionally, at 0 before the first tick, so a per-target
/// `== 0` has a series to match on — the absent-vs-zero rule the rest of
/// `worker_metrics` follows.
pub(crate) fn push_prometheus(body: &mut String) {
    body.push_str(
        "# HELP oxy_periodic_ticks_total Periodic global-driver cloud ticks run by this process. PROCESS-LOCAL: aggregate with sum across replicas.\n\
         # TYPE oxy_periodic_ticks_total counter\n",
    );
    body.push_str(&format!(
        "oxy_periodic_ticks_total {}\n",
        TICKS.load(Ordering::Relaxed)
    ));
    body.push_str(
        "# HELP oxy_periodic_workspaces_claimed_total Workspaces this process claimed and drove in its periodic ticks. PROCESS-LOCAL: the fleet-wide sum rises by the workspace count once per driver interval when the fleet keeps up.\n\
         # TYPE oxy_periodic_workspaces_claimed_total counter\n",
    );
    body.push_str(&format!(
        "oxy_periodic_workspaces_claimed_total {}\n",
        WORKSPACES_CLAIMED.load(Ordering::Relaxed)
    ));
    body.push_str(
        "# HELP oxy_periodic_reconciles_skipped_unchanged_total Per-workspace schedule reconciles skipped because the promoted revision had not changed. PROCESS-LOCAL: aggregate with sum.\n\
         # TYPE oxy_periodic_reconciles_skipped_unchanged_total counter\n",
    );
    body.push_str(&format!(
        "oxy_periodic_reconciles_skipped_unchanged_total {}\n",
        RECONCILES_SKIPPED_UNCHANGED.load(Ordering::Relaxed)
    ));
    body.push_str(
        "# HELP oxy_periodic_tick_duration_seconds Wall time of this process's last periodic tick. PER-REPLICA: read with max for the slowest node.\n\
         # TYPE oxy_periodic_tick_duration_seconds gauge\n",
    );
    body.push_str(&format!(
        "oxy_periodic_tick_duration_seconds {:.3}\n",
        LAST_TICK_MILLIS.load(Ordering::Relaxed) as f64 / 1000.0
    ));
    body.push_str(
        "# HELP oxy_periodic_due_backlog Workspaces whose periodic tick was still due and unclaimed when this process's last tick yielded (read after the drain, not before it). Zero when the tick caught up; positive only when the interval budget ran out first. DB-SOURCED, the same on every replica: aggregate with max, never sum. Staying above zero across ticks means the fleet is behind.\n\
         # TYPE oxy_periodic_due_backlog gauge\n",
    );
    body.push_str(&format!(
        "oxy_periodic_due_backlog {}\n",
        DUE_BACKLOG.load(Ordering::Relaxed)
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every series is present with a value, so a dashboard built before the
    /// first tick sees zeros rather than "no data".
    #[test]
    fn every_series_is_exported_before_the_first_tick() {
        let mut body = String::new();
        push_prometheus(&mut body);
        for metric in [
            "oxy_periodic_ticks_total ",
            "oxy_periodic_workspaces_claimed_total ",
            "oxy_periodic_reconciles_skipped_unchanged_total ",
            "oxy_periodic_tick_duration_seconds ",
            "oxy_periodic_due_backlog ",
        ] {
            assert!(body.contains(metric), "missing {metric} in:\n{body}");
        }
    }
}
