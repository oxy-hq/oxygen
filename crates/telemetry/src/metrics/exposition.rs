//! Render a [`ManualReader`] collection as Prometheus text exposition.
//!
//! ## Why this is hand-written and the aggregation is not
//!
//! `worker_metrics` hand-rolls its exposition too, and says why: a heavyweight
//! client crate is not worth it for a handful of scalars. That reasoning holds
//! for *serialization* and breaks for *aggregation* — the moment histograms
//! appear, hand-maintaining bucket boundaries, cumulative bucket counts and
//! `le` ordering under concurrency is re-implementing a metrics library in the
//! one place where a mistake silently corrupts p95.
//!
//! So the split is: the OpenTelemetry SDK owns aggregation (it already does,
//! for the OTLP reader), and this module owns only the text format, which is
//! mechanical and fully testable. The alternative — `opentelemetry-prometheus`
//! — would add a fifth crate to the lockstep group that `Cargo.toml` documents
//! as moving together at 0.32, and that crate has historically trailed the core
//! releases. A bump would then be gated on it.
//!
//! ## The translation
//!
//! Standard OpenTelemetry → Prometheus rules:
//!
//! | OTel | Prometheus |
//! | --- | --- |
//! | `http.server.request.duration`, unit `s` | `http_server_request_duration_seconds` |
//! | monotonic Sum | `…_total`, `# TYPE counter` |
//! | non-monotonic Sum (UpDownCounter) | `# TYPE gauge` |
//! | Gauge | `# TYPE gauge` |
//! | Histogram | `…_bucket{le=…}` (cumulative), `…_sum`, `…_count` |
//! | Resource attributes | one `target_info{…} 1` |
//!
//! Annotation units (`{request}`) carry no suffix, per the spec — they exist to
//! document what is counted, not to rename the series.

use std::fmt::Write as _;

use opentelemetry::KeyValue;
use opentelemetry_sdk::metrics::data::{
    AggregatedMetrics, Histogram, HistogramDataPoint, Metric, MetricData, ResourceMetrics, Sum,
};
use opentelemetry_sdk::metrics::reader::MetricReader;

/// The content type a Prometheus/VictoriaMetrics scraper expects.
pub const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Collect from `reader` and render the result as exposition text.
///
/// Returns an empty string when collection fails — the caller concatenates this
/// with other blocks (`worker_metrics`' hand-rolled set), and losing one block
/// must not cost the others. The failure is logged rather than propagated for
/// the same reason `oxy_metrics_scrape_db_ok` exists: a scrape that returns
/// partial data beats a scrape that returns a 500.
pub fn render<R: MetricReader>(reader: &R) -> String {
    let mut rm = ResourceMetrics::default();
    if let Err(err) = reader.collect(&mut rm) {
        tracing::warn!(target: "oxy_telemetry::metrics", ?err, "metrics collection failed");
        return String::new();
    }
    render_resource_metrics(&rm)
}

/// The pure half of [`render`], so tests can drive it from a collected
/// [`ResourceMetrics`] without a live reader.
pub(crate) fn render_resource_metrics(rm: &ResourceMetrics) -> String {
    let mut out = String::with_capacity(8 * 1024);
    push_target_info(&mut out, rm);
    for scope in rm.scope_metrics() {
        for metric in scope.metrics() {
            push_metric(&mut out, metric);
        }
    }
    out
}

/// `target_info` carries the resource attributes — `service.name`, the fleet
/// role, version, environment — that every other series would otherwise have to
/// repeat. The standard OTel-to-Prometheus shape, and what lets a dashboard
/// join a series to the process that emitted it.
fn push_target_info(out: &mut String, rm: &ResourceMetrics) {
    let labels: Vec<(String, String)> = rm
        .resource()
        .iter()
        .map(|(k, v)| (sanitize_label(k.as_str()), v.to_string()))
        .collect();
    if labels.is_empty() {
        return;
    }
    out.push_str("# HELP target_info Attributes of the process that emitted these series.\n");
    out.push_str("# TYPE target_info gauge\n");
    out.push_str("target_info");
    push_label_set(out, &labels);
    out.push_str(" 1\n");
}

fn push_metric(out: &mut String, metric: &Metric) {
    match metric.data() {
        AggregatedMetrics::F64(data) => push_metric_data(out, metric, data, f64_to_string),
        AggregatedMetrics::U64(data) => push_metric_data(out, metric, data, u64_to_string),
        AggregatedMetrics::I64(data) => push_metric_data(out, metric, data, i64_to_string),
    }
}

fn push_metric_data<T: Copy>(
    out: &mut String,
    metric: &Metric,
    data: &MetricData<T>,
    fmt: fn(T) -> String,
) {
    match data {
        MetricData::Gauge(gauge) => {
            let name = prom_name(metric.name(), metric.unit(), false);
            push_header(out, &name, metric.description(), "gauge");
            for point in gauge.data_points() {
                push_sample(out, &name, &attrs(point.attributes()), &fmt(point.value()));
            }
        }
        MetricData::Sum(sum) => push_sum(out, metric, sum, fmt),
        MetricData::Histogram(hist) => push_histogram(out, metric, hist, fmt),
        MetricData::ExponentialHistogram(_) => {
            // No instrument here uses one, and the classic text format has no
            // representation for it. Skipping beats emitting something a
            // scraper would misread as a different distribution.
            tracing::debug!(
                target: "oxy_telemetry::metrics",
                metric = metric.name(),
                "exponential histogram skipped: unrepresentable in text exposition"
            );
        }
    }
}

fn push_sum<T: Copy>(out: &mut String, metric: &Metric, sum: &Sum<T>, fmt: fn(T) -> String) {
    // A monotonic sum is a Prometheus counter and takes `_total`; a
    // non-monotonic one is an UpDownCounter, which is a gauge and must not.
    let monotonic = sum.is_monotonic();
    let name = prom_name(metric.name(), metric.unit(), monotonic);
    let kind = if monotonic { "counter" } else { "gauge" };
    push_header(out, &name, metric.description(), kind);
    for point in sum.data_points() {
        push_sample(out, &name, &attrs(point.attributes()), &fmt(point.value()));
    }
}

fn push_histogram<T: Copy>(
    out: &mut String,
    metric: &Metric,
    hist: &Histogram<T>,
    fmt: fn(T) -> String,
) {
    let name = prom_name(metric.name(), metric.unit(), false);
    push_header(out, &name, metric.description(), "histogram");
    for point in hist.data_points() {
        push_histogram_point(out, &name, point, fmt);
    }
}

fn push_histogram_point<T: Copy>(
    out: &mut String,
    name: &str,
    point: &HistogramDataPoint<T>,
    fmt: fn(T) -> String,
) {
    let labels = attrs(point.attributes());
    let bounds: Vec<f64> = point.bounds().collect();
    let counts: Vec<u64> = point.bucket_counts().collect();

    // OTel bucket counts are per-bucket and there is one more of them than
    // there are bounds (the overflow). Prometheus wants them cumulative, with a
    // final `+Inf` equal to the total count.
    let mut cumulative = 0u64;
    for (idx, bound) in bounds.iter().enumerate() {
        cumulative += counts.get(idx).copied().unwrap_or(0);
        let mut bucket_labels = labels.clone();
        bucket_labels.push(("le".to_owned(), f64_to_string(*bound)));
        push_sample(
            out,
            &format!("{name}_bucket"),
            &bucket_labels,
            &cumulative.to_string(),
        );
    }
    let mut inf_labels = labels.clone();
    inf_labels.push(("le".to_owned(), "+Inf".to_owned()));
    push_sample(
        out,
        &format!("{name}_bucket"),
        &inf_labels,
        &point.count().to_string(),
    );

    push_sample(out, &format!("{name}_sum"), &labels, &fmt(point.sum()));
    push_sample(
        out,
        &format!("{name}_count"),
        &labels,
        &point.count().to_string(),
    );
}

fn push_header(out: &mut String, name: &str, description: &str, kind: &str) {
    if !description.is_empty() {
        let _ = writeln!(out, "# HELP {name} {}", escape_help(description));
    }
    let _ = writeln!(out, "# TYPE {name} {kind}");
}

fn push_sample(out: &mut String, name: &str, labels: &[(String, String)], value: &str) {
    out.push_str(name);
    push_label_set(out, labels);
    out.push(' ');
    out.push_str(value);
    out.push('\n');
}

fn push_label_set(out: &mut String, labels: &[(String, String)]) {
    if labels.is_empty() {
        return;
    }
    out.push('{');
    for (idx, (key, value)) in labels.iter().enumerate() {
        if idx > 0 {
            out.push(',');
        }
        out.push_str(key);
        out.push_str("=\"");
        out.push_str(&escape_label_value(value));
        out.push('"');
    }
    out.push('}');
}

fn attrs<'a>(kvs: impl Iterator<Item = &'a KeyValue>) -> Vec<(String, String)> {
    let mut labels: Vec<(String, String)> = kvs
        .map(|kv| (sanitize_label(kv.key.as_str()), kv.value.to_string()))
        .collect();
    // Sorted so a series' label order is stable between scrapes. Prometheus
    // does not require it, but a diffable /metrics body makes debugging a
    // cardinality surprise far easier.
    labels.sort_by(|a, b| a.0.cmp(&b.0));
    labels
}

/// `http.server.request.duration` + unit `s` → `http_server_request_duration_seconds`.
fn prom_name(otel_name: &str, unit: &str, monotonic: bool) -> String {
    let mut name = sanitize_label(otel_name);
    if let Some(suffix) = unit_suffix(unit) {
        if !name.ends_with(suffix) {
            name.push('_');
            name.push_str(suffix);
        }
    }
    if monotonic && !name.ends_with("_total") {
        name.push_str("_total");
    }
    name
}

/// The unit suffix a Prometheus name carries, if any.
///
/// Annotation units — anything in curly braces, like `{request}` — describe
/// what is being counted rather than a physical unit, and carry no suffix.
fn unit_suffix(unit: &str) -> Option<&'static str> {
    match unit {
        "s" => Some("seconds"),
        "ms" => Some("milliseconds"),
        "By" => Some("bytes"),
        _ => None,
    }
}

/// Prometheus names and label keys admit `[a-zA-Z0-9_:]`; everything else
/// becomes `_`. A leading digit gets an underscore prefix, which is the only
/// case where the sanitized name is longer by more than a substitution.
fn sanitize_label(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (idx, ch) in raw.chars().enumerate() {
        let ok = ch.is_ascii_alphanumeric() || ch == '_' || ch == ':';
        if ok && !(idx == 0 && ch.is_ascii_digit()) {
            out.push(ch);
        } else if idx == 0 && ch.is_ascii_digit() {
            out.push('_');
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn escape_label_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// HELP text escapes backslash and newline, but **not** the double quote — the
/// quote is only special inside a label value. Escaping it here would put a
/// literal backslash in the rendered help.
fn escape_help(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// Prometheus wants `+Inf` / `NaN` spelled that way, and an integral float
/// without a trailing `.0` so `le="1"` matches what a scraper expects.
fn f64_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value.is_sign_positive() {
            "+Inf".to_owned()
        } else {
            "-Inf".to_owned()
        };
    }
    if value == value.trunc() && value.abs() < 1e15 {
        return format!("{}", value as i64);
    }
    format!("{value}")
}

fn u64_to_string(value: u64) -> String {
    value.to_string()
}

fn i64_to_string(value: i64) -> String {
    value.to_string()
}

#[cfg(test)]
#[path = "exposition_tests.rs"]
mod tests;
