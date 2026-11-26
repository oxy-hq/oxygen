//! The pool health probe's two sync instruments, recorded through the
//! OpenTelemetry API's **global** meter rather than through `oxy-telemetry`,
//! so this crate need not depend on the metrics runtime
//! (`internal-docs/crate-tiers.md`).
//!
//! The instruments' identity (name, description, unit, buckets, the
//! `oxy.reason` label) is the kernel's, `oxy_shared::db_pool_metrics`, and
//! `oxy-telemetry` builds the same two instruments from it under the same
//! scope to seed their zero series. Same identity, same scope: the SDK resolves
//! both to one stream. `the_probe_records_into_telemetry_s_one_stream` below
//! pins that against the real provider.
//!
//! The instruments are looked up on every record, never cached: the global
//! meter is a no-op until telemetry installs its provider
//! (`global::set_meter_provider`), and an instrument taken from it before then
//! would stay a no-op forever. Before telemetry installs, a record goes
//! nowhere, exactly as it did through telemetry's `with_instruments`. The
//! probe records once per `POOL_HEALTH_INTERVAL`, so the lookup costs nothing.
//!
//! The gauges (occupancy, ceiling, starvation) are plain atomics in the same
//! kernel module, which telemetry's observable callbacks read.

use opentelemetry::KeyValue;
use opentelemetry::global;
use oxy_shared::db_pool_metrics as def;

/// One successful pool health probe, and how long it waited for a connection.
pub(super) fn probe(seconds: f64) {
    global::meter(def::METER_SCOPE)
        .f64_histogram(def::PROBE_DURATION_NAME)
        .with_description(def::PROBE_DURATION_DESCRIPTION)
        .with_unit(def::PROBE_DURATION_UNIT)
        .with_boundaries(def::PROBE_DURATION_BUCKETS.to_vec())
        .build()
        .record(seconds, &[]);
}

/// One pool health probe that did not acquire.
///
/// `reason` is one of `DB_POOL_PROBE_FAILURE_REASONS`: `timeout` (the pool was
/// exhausted), `server_unavailable` (the pool had room and the wait was on
/// Postgres) or `error` (the checkout failed outright). None records a
/// duration, so the histogram's `_count` and this counter differ by exactly
/// the probes that did not acquire.
pub(super) fn probe_failure(reason: &'static str) {
    global::meter(def::METER_SCOPE)
        .u64_counter(def::PROBE_FAILURES_NAME)
        .with_description(def::PROBE_FAILURES_DESCRIPTION)
        .with_unit(def::PROBE_FAILURES_UNIT)
        .build()
        .add(1, &[KeyValue::new(def::PROBE_FAILURE_REASON_KEY, reason)]);
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oxy_telemetry::metrics::{MetricsConfig, init, render_prometheus};

    use super::*;

    fn lines<'a>(body: &'a str, prefix: &str) -> Vec<&'a str> {
        body.lines().filter(|l| l.starts_with(prefix)).collect()
    }

    /// The probe's records land in the same series telemetry defines and
    /// seeds: one `# TYPE` per metric (a mismatched identity would register a
    /// second, conflicting instrument), the histogram's buckets are the
    /// kernel's, and the failure counter's seeded zeros are the series the
    /// probe increments (a decoy would show two `timeout` lines).
    #[test]
    fn the_probe_records_into_telemetry_s_one_stream() {
        let problems = init(
            &MetricsConfig {
                sdk_disabled: false,
                otlp_enabled: false,
                otlp_endpoint: None,
                export_interval: Duration::from_secs(60),
            },
            opentelemetry_sdk::Resource::builder_empty().build(),
        );
        assert!(problems.is_empty(), "{problems:?}");

        probe(0.003);
        probe_failure(def::DB_POOL_PROBE_FAILURE_TIMEOUT);
        let body = render_prometheus();

        assert_eq!(
            lines(&body, "# TYPE oxy_db_pool_probe_duration_seconds "),
            ["# TYPE oxy_db_pool_probe_duration_seconds histogram"],
            "{body}"
        );
        let le: Vec<String> = lines(&body, "oxy_db_pool_probe_duration_seconds_bucket")
            .iter()
            .filter_map(|l| {
                l.split("le=\"")
                    .nth(1)?
                    .split('"')
                    .next()
                    .map(str::to_owned)
            })
            .collect();
        let mut want: Vec<String> = def::PROBE_DURATION_BUCKETS
            .iter()
            .map(|b| b.to_string())
            .collect();
        want.push("+Inf".into());
        assert_eq!(le, want, "{body}");
        assert_eq!(
            lines(&body, "oxy_db_pool_probe_duration_seconds_count"),
            ["oxy_db_pool_probe_duration_seconds_count 1"],
            "{body}"
        );

        assert_eq!(
            lines(&body, "# TYPE oxy_db_pool_probe_failures_total "),
            ["# TYPE oxy_db_pool_probe_failures_total counter"],
            "{body}"
        );
        let failures = lines(&body, "oxy_db_pool_probe_failures_total{");
        assert_eq!(failures.len(), 3, "one series per reason, no decoy: {body}");
        assert!(
            failures.contains(&r#"oxy_db_pool_probe_failures_total{oxy_reason="timeout"} 1"#),
            "{body}"
        );
        assert!(
            failures.contains(&r#"oxy_db_pool_probe_failures_total{oxy_reason="error"} 0"#),
            "{body}"
        );
    }
}
