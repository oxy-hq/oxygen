//! Exposition tests, driven through a **real** `SdkMeterProvider`.
//!
//! These deliberately do not hand-build `ResourceMetrics`. The whole argument
//! for this design is that the SDK owns aggregation and this module owns only
//! serialization; a test that fabricated the aggregated form would verify the
//! half we did not write and skip the seam where the two meet.

use opentelemetry::KeyValue;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::SdkMeterProvider;

use super::*;
use crate::metrics::SharedManualReader;

/// A provider wired exactly as production wires it, minus the OTLP reader.
fn harness() -> (SdkMeterProvider, SharedManualReader) {
    let reader = SharedManualReader::new();
    let provider = SdkMeterProvider::builder()
        .with_resource(
            Resource::builder_empty()
                .with_attributes([
                    KeyValue::new("service.name", "oxy-serve"),
                    KeyValue::new("oxy.role", "serve"),
                ])
                .build(),
        )
        .with_reader(reader.clone())
        .build();
    (provider, reader)
}

/// Pull one metric's block out of the rendered body, so an assertion is not
/// accidentally satisfied by a different series' text.
fn lines_for(body: &str, prefix: &str) -> Vec<String> {
    body.lines()
        .filter(|l| l.starts_with(prefix) && !l.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

#[test]
fn histogram_buckets_are_cumulative_and_inf_equals_count() {
    let (provider, reader) = harness();
    let hist = provider
        .meter("test")
        .f64_histogram("oxy.test.duration")
        .with_unit("s")
        .with_boundaries(vec![0.05, 0.5, 1.0])
        .build();

    hist.record(0.01, &[]);
    hist.record(0.2, &[]);
    hist.record(5.0, &[]);

    let body = render(&reader);
    let buckets = lines_for(&body, "oxy_test_duration_seconds_bucket");

    assert_eq!(
        buckets,
        vec![
            r#"oxy_test_duration_seconds_bucket{le="0.05"} 1"#,
            r#"oxy_test_duration_seconds_bucket{le="0.5"} 2"#,
            r#"oxy_test_duration_seconds_bucket{le="1"} 2"#,
            r#"oxy_test_duration_seconds_bucket{le="+Inf"} 3"#,
        ],
        "OTel reports per-bucket counts; Prometheus needs them accumulated. \
         Getting this wrong makes every histogram_quantile() silently wrong.\n{body}"
    );

    assert!(
        body.contains("oxy_test_duration_seconds_count 3"),
        "missing _count in:\n{body}"
    );
    assert!(
        body.contains("oxy_test_duration_seconds_sum 5.21"),
        "missing or wrong _sum in:\n{body}"
    );
}

#[test]
fn the_inf_bucket_tracks_count_even_when_nothing_overflows() {
    let (provider, reader) = harness();
    let hist = provider
        .meter("test")
        .f64_histogram("oxy.test.inrange")
        .with_unit("s")
        .with_boundaries(vec![1.0, 10.0])
        .build();
    hist.record(0.5, &[]);
    hist.record(0.5, &[]);

    let body = render(&reader);
    assert!(
        body.contains(r#"oxy_test_inrange_seconds_bucket{le="+Inf"} 2"#),
        "+Inf must always equal _count, overflow or not:\n{body}"
    );
}

#[test]
fn a_monotonic_counter_gets_total_and_an_updown_counter_does_not() {
    let (provider, reader) = harness();
    let meter = provider.meter("test");

    meter
        .u64_counter("oxy.test.things")
        .with_unit("{thing}")
        .build()
        .add(3, &[]);
    meter
        .i64_up_down_counter("oxy.test.inflight")
        .with_unit("{thing}")
        .build()
        .add(2, &[]);

    let body = render(&reader);

    assert!(
        body.contains("# TYPE oxy_test_things_total counter"),
        "{body}"
    );
    assert!(body.contains("oxy_test_things_total 3"), "{body}");

    // An UpDownCounter is a gauge. `_total` on it would make `rate()` look
    // legitimate on a series that can decrease.
    assert!(
        body.contains("# TYPE oxy_test_inflight gauge"),
        "an UpDownCounter must render as a gauge:\n{body}"
    );
    assert!(
        !body.contains("oxy_test_inflight_total"),
        "a non-monotonic sum must not get _total:\n{body}"
    );
}

#[test]
fn annotation_units_add_no_suffix_but_real_units_do() {
    let (provider, reader) = harness();
    let meter = provider.meter("test");

    meter
        .u64_counter("oxy.test.calls")
        .with_unit("{call}")
        .build()
        .add(1, &[]);
    meter
        .u64_counter("oxy.test.payload")
        .with_unit("By")
        .build()
        .add(1, &[]);

    let body = render(&reader);
    assert!(
        body.contains("oxy_test_calls_total 1"),
        "{{call}} is an annotation, not a unit — it must not become a suffix:\n{body}"
    );
    assert!(
        body.contains("oxy_test_payload_bytes_total 1"),
        "By must render as _bytes:\n{body}"
    );
}

#[test]
fn labels_are_sorted_and_escaped() {
    let (provider, reader) = harness();
    provider
        .meter("test")
        .u64_counter("oxy.test.labelled")
        .with_unit("{thing}")
        .build()
        .add(
            1,
            &[
                KeyValue::new("zebra", "last"),
                KeyValue::new("http.route", "/a/{id}"),
                KeyValue::new("quoted", r#"he said "hi"\"#),
            ],
        );

    let body = render(&reader);
    let line = lines_for(&body, "oxy_test_labelled_total")
        .pop()
        .unwrap_or_else(|| panic!("no series in:\n{body}"));

    assert_eq!(
        line,
        r#"oxy_test_labelled_total{http_route="/a/{id}",quoted="he said \"hi\"\\",zebra="last"} 1"#,
        "dots become underscores, values are escaped, keys sort"
    );
}

#[test]
fn resource_attributes_become_target_info() {
    let (_provider, reader) = harness();
    let body = render(&reader);

    assert!(body.contains("# TYPE target_info gauge"), "{body}");
    assert!(
        body.contains(r#"service_name="oxy-serve""#),
        "the resource is how a series is attributed to a fleet role:\n{body}"
    );
    assert!(body.contains(r#"oxy_role="serve""#), "{body}");
}

#[test]
fn help_text_escapes_backslashes_but_leaves_quotes_alone() {
    // A quote is only special inside a label value. Escaping it in HELP would
    // put a literal backslash into the operator-facing description.
    assert_eq!(escape_help(r#"say "hi""#), r#"say "hi""#);
    assert_eq!(escape_help(r"a\b"), r"a\\b");
    assert_eq!(escape_help("two\nlines"), r"two\nlines");
}

#[test]
fn names_are_sanitized_and_never_start_with_a_digit() {
    assert_eq!(
        sanitize_label("http.server.duration"),
        "http_server_duration"
    );
    assert_eq!(sanitize_label("weird-key!"), "weird_key_");
    assert_eq!(
        sanitize_label("5xx"),
        "_5xx",
        "a leading digit is not a legal Prometheus name"
    );
}

#[test]
fn a_unit_suffix_is_not_applied_twice() {
    // `oxy.thing.seconds` with unit `s` must not become `..._seconds_seconds`.
    assert_eq!(
        prom_name("oxy.thing.seconds", "s", false),
        "oxy_thing_seconds"
    );
    assert_eq!(prom_name("oxy.thing", "s", false), "oxy_thing_seconds");
    assert_eq!(prom_name("oxy.thing_total", "", true), "oxy_thing_total");
}

#[test]
fn integral_floats_render_without_a_trailing_point_zero() {
    // `le="1.0"` is not what a scraper writes, and a bucket label that differs
    // textually between exporters breaks recording rules that match on it.
    assert_eq!(f64_to_string(1.0), "1");
    assert_eq!(f64_to_string(0.05), "0.05");
    assert_eq!(f64_to_string(f64::INFINITY), "+Inf");
    assert_eq!(f64_to_string(f64::NAN), "NaN");
}

#[test]
fn an_empty_provider_still_renders_valid_text() {
    let (_provider, reader) = harness();
    let body = render(&reader);
    // Only target_info. The important property is that a scraper gets a
    // parseable body rather than an empty one it reports as a failed scrape.
    assert!(body.starts_with("# HELP target_info"), "{body}");
    assert!(body.ends_with('\n'), "exposition must end with a newline");
}

/// Every instrument's rendered Prometheus type, pinned.
///
/// `oxy.db.pool.starvation_events` shipped as an `observable_gauge` while its
/// own description said "Monotonic" and the operator doc listed it as a
/// counter. It therefore rendered `# TYPE … gauge` with no `_total` suffix, so
/// a rule written from the doc — `increase(…_total[1h])` — matched a series
/// that did not exist. That is an alert that can never fire, which is the
/// failure class this whole module exists to close, so it gets a test that
/// covers *every* instrument rather than the one that was wrong.
#[test]
fn every_instrument_renders_with_its_intended_prometheus_type() {
    use crate::metrics::Instruments;
    use std::collections::BTreeMap;

    let (provider, reader) = harness();
    let i = Instruments::new(&provider.meter("oxy"));

    // OTel exports only instruments that hold data, so every synchronous one
    // needs a sample — without this the assertions below pass vacuously.
    // Observables export from their callbacks and need nothing.
    i.http_request_duration.record(0.1, &[]);
    i.http_active_requests.add(1, &[]);
    i.db_pool_probe_duration.record(0.001, &[]);
    i.db_pool_probe_failures.add(1, &[]);
    i.custom_app_request_duration.record(0.1, &[]);
    i.custom_app_function_duration.record(0.1, &[]);
    i.custom_app_function_init_duration.record(0.001, &[]);
    i.custom_app_function_host_calls.record(1.0, &[]);
    i.custom_app_function_invocations.add(1, &[]);
    i.custom_app_heap_terminations.add(1, &[]);
    i.custom_app_admission_wait.record(0.001, &[]);
    i.custom_app_admission_shed.add(1, &[]);
    i.custom_app_bundle_cache_evictions.add(1, &[]);

    let body = render(&reader);
    let types: BTreeMap<&str, &str> = body
        .lines()
        .filter_map(|l| l.strip_prefix("# TYPE ")?.rsplit_once(' '))
        .collect();

    for (name, expected) in [
        ("http_server_request_duration_seconds", "histogram"),
        ("http_server_active_requests", "gauge"),
        ("oxy_db_pool_probe_duration_seconds", "histogram"),
        ("oxy_db_pool_probe_failures_total", "counter"),
        ("oxy_db_pool_connections", "gauge"),
        ("oxy_db_pool_max", "gauge"),
        ("oxy_db_pool_starved", "gauge"),
        ("oxy_db_pool_starvation_events_total", "counter"),
        ("oxy_custom_app_request_duration_seconds", "histogram"),
        ("oxy_custom_app_function_duration_seconds", "histogram"),
        ("oxy_custom_app_function_init_duration_seconds", "histogram"),
        ("oxy_custom_app_function_host_calls", "histogram"),
        ("oxy_custom_app_function_invocations_total", "counter"),
        ("oxy_custom_app_isolates_live", "gauge"),
        ("oxy_custom_app_isolates_live_peak", "gauge"),
        ("oxy_custom_app_isolates_abandoned_total", "counter"),
        ("oxy_custom_app_isolates_heap_terminations_total", "counter"),
        ("oxy_custom_app_admission_in_use", "gauge"),
        ("oxy_custom_app_admission_limit", "gauge"),
        ("oxy_custom_app_admission_queued", "gauge"),
        ("oxy_custom_app_admission_wait_seconds", "histogram"),
        ("oxy_custom_app_admission_shed_total", "counter"),
        // `…bundle_cache.bytes` already ends in the unit, so the `By` suffix
        // is not applied twice; `…bundle_cache.limit` does not, so it gains
        // `_bytes`. The asymmetry is the semconv rule working, not a typo.
        ("oxy_custom_app_bundle_cache_bytes", "gauge"),
        ("oxy_custom_app_bundle_cache_limit_bytes", "gauge"),
        ("oxy_custom_app_bundle_cache_evictions_total", "counter"),
    ] {
        assert_eq!(
            types.get(name),
            Some(&expected),
            "{name} must render as {expected}. A monotonic value registered as \
             a gauge loses its _total suffix and rejects rate()/increase(); a \
             gauge registered as a counter gains a suffix nobody queries.\n{body}"
        );
    }
}

#[test]
fn every_rendered_line_is_a_comment_or_a_sample() {
    // A cheap structural check over the whole body: the format admits exactly
    // two line shapes, and a stray line breaks the entire scrape, not one
    // series.
    let (provider, reader) = harness();
    let meter = provider.meter("test");
    meter
        .f64_histogram("oxy.test.shape")
        .with_unit("s")
        .with_boundaries(vec![0.1, 1.0])
        .build()
        .record(0.5, &[KeyValue::new("k", "v")]);
    meter
        .u64_counter("oxy.test.shape_count")
        .with_unit("{x}")
        .build()
        .add(1, &[]);

    for line in render(&reader).lines() {
        if line.starts_with('#') {
            assert!(
                line.starts_with("# HELP ") || line.starts_with("# TYPE "),
                "unknown comment form: {line}"
            );
            continue;
        }
        let (_name, value) = line
            .rsplit_once(' ')
            .unwrap_or_else(|| panic!("sample line has no value: {line}"));
        assert!(
            value.parse::<f64>().is_ok() || value == "+Inf" || value == "NaN",
            "sample value is not a number: {line}"
        );
    }
}

/// The premise the seeding rests on: a synchronous counter that nothing has
/// recorded into exports **no series at all**. If OTel ever changed that, the
/// seeding below would be unnecessary rather than merely harmless, and this is
/// the test that would say so.
#[test]
fn an_unrecorded_sync_counter_exports_nothing() {
    use crate::metrics::Instruments;

    let (provider, reader) = harness();
    let _instruments = Instruments::new(&provider.meter("oxy"));

    let body = render(&reader);
    assert!(
        !body.contains("oxy_custom_app_bundle_cache_evictions"),
        "a sync counter with no samples should export nothing — if it now \
         exports a zero on its own, `record::seed_zero_series` is redundant:\n{body}"
    );
}

/// Seeding gives the alertable sync counters a prior sample, so the first real
/// event is a visible delta rather than the series' birth value.
///
/// Both halves are the assertion. The counters that CAN be seeded must be
/// present at zero; the org-labelled ones must be absent, because a seed under
/// an attribute set the real record will not use is a decoy pinned at 0 while
/// the true series is born beside it.
#[test]
fn seeding_makes_a_first_event_visible_to_increase() {
    use crate::metrics::Instruments;

    let (provider, reader) = harness();
    let instruments = Instruments::new(&provider.meter("oxy"));
    crate::metrics::record::seed_zero_series(&instruments);

    let body = render(&reader);

    for name in ["oxy_custom_app_bundle_cache_evictions_total".to_owned()]
        .into_iter()
        .chain(probe_failure_series())
    {
        let line = body
            .lines()
            .find(|l| l.starts_with(name.as_str()) && !l.starts_with('#'))
            .unwrap_or_else(|| panic!("seeded series is missing: {name}\n{body}"));
        let (_, value) = line.rsplit_once(' ').expect("sample has a value");
        assert_eq!(value, "0", "a seed must be zero, not a count: {line}");
    }

    for absent in [
        "oxy_custom_app_admission_shed_total",
        "oxy_custom_app_isolates_heap_terminations_total",
        "oxy_custom_app_function_invocations_total",
    ] {
        assert!(
            !body.contains(absent),
            "{absent} is keyed by org, so it must NOT be seeded — a seed under \
             the wrong attribute set is a decoy that reads calm while the real \
             series is born elsewhere:\n{body}"
        );
    }
}

/// One exposition name per pool-probe reason, spelled from the constants.
///
/// Built from each **named** constant rather than by iterating
/// `DB_POOL_PROBE_FAILURE_REASONS`: iterating the list would pass for a
/// constant the probe records but the list forgot, which is exactly the
/// un-seeded series this exists to rule out.
fn probe_failure_series() -> Vec<String> {
    use crate::metrics::record::{
        DB_POOL_PROBE_FAILURE_ERROR, DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE,
        DB_POOL_PROBE_FAILURE_TIMEOUT,
    };
    [
        DB_POOL_PROBE_FAILURE_TIMEOUT,
        DB_POOL_PROBE_FAILURE_SERVER_UNAVAILABLE,
        DB_POOL_PROBE_FAILURE_ERROR,
    ]
    .iter()
    .map(|reason| format!(r#"oxy_db_pool_probe_failures_total{{oxy_reason="{reason}"}}"#))
    .collect()
}

/// The seeded list is exactly the named reasons — no more, no fewer.
///
/// A new reason has to be added in three places: its constant, the list the
/// seed walks, and [`probe_failure_series`]. The length pin fails when the list
/// grows without the helper; the helper's assertion above fails when a named
/// constant is missing from the list's seed.
#[test]
fn probe_failure_reasons_are_the_named_constants() {
    use crate::metrics::record::DB_POOL_PROBE_FAILURE_REASONS;

    let named = probe_failure_series();
    assert_eq!(
        DB_POOL_PROBE_FAILURE_REASONS.len(),
        named.len(),
        "DB_POOL_PROBE_FAILURE_REASONS and probe_failure_series() disagree on how many \
         reasons exist: {DB_POOL_PROBE_FAILURE_REASONS:?} vs {named:?}"
    );
    let mut distinct = DB_POOL_PROBE_FAILURE_REASONS.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    assert_eq!(
        distinct.len(),
        DB_POOL_PROBE_FAILURE_REASONS.len(),
        "a reason is listed twice, so another is not seeded: {DB_POOL_PROBE_FAILURE_REASONS:?}"
    );
}
