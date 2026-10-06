//! Latency-percentile, histogram, and per-model cost queries.
//!
//! Percentiles and the histogram are plain `quantile`/bucket aggregations over
//! `observability_executions.duration_ns`. Cost aggregates `llm` spans directly
//! (tokens live on the `llm.usage` event, model on `gen_ai.request.model`) —
//! the executions rollup carries no token data, so this reads `observability_spans`.

use clickhouse::Row;
use oxy_shared::errors::OxyError;
use serde::Deserialize;

use super::ClickHouseObservabilityStorage;
use crate::scope::WorkspaceScope;
use crate::types::{
    HistogramBucketData, LatencyHistogramData, LatencyPercentilePoint, LatencyPercentiles,
    LatencyPercentilesData, ModelUsageData,
};

/// ClickHouse `quantile` on an empty set yields NaN; panels want 0.
fn finite(x: f64) -> f64 {
    if x.is_finite() { x } else { 0.0 }
}

#[derive(Debug, Deserialize, Row)]
struct TripleRow {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

#[derive(Debug, Deserialize, Row)]
struct SeriesRow {
    date: String,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
}

#[derive(Debug, Deserialize, Row)]
struct BucketRow {
    bucket: u16,
    count: u64,
}

#[derive(Debug, Deserialize, Row)]
struct ModelUsageRow {
    model: String,
    calls: u64,
    input_tokens: i64,
    output_tokens: i64,
    p95_ms: f64,
}

const QUANTILES: &str = "\
    quantile(0.5)(duration_ns) / 1000000.0 AS p50_ms, \
    quantile(0.95)(duration_ns) / 1000000.0 AS p95_ms, \
    quantile(0.99)(duration_ns) / 1000000.0 AS p99_ms";

async fn overall_percentiles(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
) -> Result<LatencyPercentiles, OxyError> {
    let in_scope = scope.rollup_predicate(days);
    let sql = format!(
        "SELECT {QUANTILES} FROM observability_executions FINAL \
         WHERE timestamp >= now() - INTERVAL {days} DAY AND {in_scope}"
    );
    let row = storage
        .read_client()
        .query(&sql)
        .fetch_optional::<TripleRow>()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Latency percentiles query failed: {e}")))?;
    Ok(row
        .map(|r| LatencyPercentiles {
            p50_ms: finite(r.p50_ms),
            p95_ms: finite(r.p95_ms),
            p99_ms: finite(r.p99_ms),
        })
        .unwrap_or_default())
}

pub(super) async fn get_latency_percentiles(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
) -> Result<LatencyPercentilesData, OxyError> {
    let in_scope = scope.rollup_predicate(days);
    let overall = overall_percentiles(storage, scope, days).await?;

    let series_sql = format!(
        "SELECT formatDateTime(toDate(timestamp), '%Y-%m-%d') AS date, {QUANTILES} \
         FROM observability_executions FINAL \
         WHERE timestamp >= now() - INTERVAL {days} DAY AND {in_scope} \
         GROUP BY date ORDER BY date ASC"
    );
    let rows: Vec<SeriesRow> = storage
        .read_client()
        .query(&series_sql)
        .fetch_all()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Latency series query failed: {e}")))?;

    let series = rows
        .into_iter()
        .map(|r| LatencyPercentilePoint {
            date: r.date,
            p50_ms: finite(r.p50_ms),
            p95_ms: finite(r.p95_ms),
            p99_ms: finite(r.p99_ms),
        })
        .collect();

    Ok(LatencyPercentilesData { overall, series })
}

pub(super) async fn get_latency_histogram(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
) -> Result<LatencyHistogramData, OxyError> {
    let in_scope = scope.rollup_predicate(days);
    // Log2 buckets clamped to [0, 15]; bucket b holds durations in
    // (2^b, 2^(b+1)] ms, so its inclusive upper bound is 2^(b+1) ms.
    let sql = format!(
        "SELECT
            toUInt16(least(15, greatest(0, toInt32(floor(log2(greatest(duration_ns / 1000000.0, 1.0))))))) AS bucket,
            count() AS count
        FROM observability_executions FINAL
        WHERE timestamp >= now() - INTERVAL {days} DAY AND {in_scope}
        GROUP BY bucket ORDER BY bucket ASC"
    );
    let rows: Vec<BucketRow> = storage
        .read_client()
        .query(&sql)
        .fetch_all()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Latency histogram query failed: {e}")))?;

    let buckets = rows
        .into_iter()
        .map(|r| HistogramBucketData {
            upper_ms: 2f64.powi(i32::from(r.bucket) + 1),
            count: r.count,
        })
        .collect();

    let percentiles = overall_percentiles(storage, scope, days).await?;
    Ok(LatencyHistogramData {
        buckets,
        percentiles,
    })
}

pub(super) async fn get_model_usage(
    storage: &ClickHouseObservabilityStorage,
    scope: &WorkspaceScope,
    days: u32,
) -> Result<Vec<ModelUsageData>, OxyError> {
    let on_spans = scope.spans_predicate("");
    // Tokens are stringified event fields on the `llm.usage` event; model is a
    // span attribute. Extract per span, then aggregate per model.
    let sql = format!(
        "SELECT
            model,
            count() AS calls,
            sum(input_tokens) AS input_tokens,
            sum(output_tokens) AS output_tokens,
            quantile(0.95)(duration_ns) / 1000000.0 AS p95_ms
        FROM (
            SELECT
                JSONExtractString(span_attributes, 'gen_ai.request.model') AS model,
                toInt64OrZero(JSONExtractString(arrayFirst(
                    x -> JSONExtractString(x, 'name') = 'llm.usage',
                    JSONExtractArrayRaw(event_data)), 'attributes', 'prompt_tokens')) AS input_tokens,
                toInt64OrZero(JSONExtractString(arrayFirst(
                    x -> JSONExtractString(x, 'name') = 'llm.usage',
                    JSONExtractArrayRaw(event_data)), 'attributes', 'completion_tokens')) AS output_tokens,
                duration_ns
            FROM observability_spans
            WHERE {on_spans}
              AND JSONExtractString(span_attributes, 'oxy.span_type') = 'llm'
              AND timestamp >= now() - INTERVAL {days} DAY
        )
        WHERE model != ''
        GROUP BY model
        ORDER BY calls DESC"
    );
    let rows: Vec<ModelUsageRow> = storage
        .read_client()
        .query(&sql)
        .fetch_all()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Model usage query failed: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| ModelUsageData {
            model: r.model,
            calls: r.calls,
            input_tokens: r.input_tokens.max(0) as u64,
            output_tokens: r.output_tokens.max(0) as u64,
            p95_ms: finite(r.p95_ms),
        })
        .collect())
}
