use agentic_core::tools::ToolError;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::anomaly_store::{
    AnomalyFilter, AnomalyRecord, AnomalyStore, AnomalyStoreError, DetectRequest,
};
use crate::metric_tree_runner::MetricTreeRunner;

use super::{emit_tool_error, emit_tool_input, emit_tool_output};

pub struct AnomalyToolContext<'a> {
    pub workspace_id: Uuid,
    pub store: &'a dyn AnomalyStore,
    pub runner: &'a dyn MetricTreeRunner,
}

pub async fn execute_anomaly_tool(
    name: &str,
    params: Value,
    ctx: &AnomalyToolContext<'_>,
) -> Result<Value, ToolError> {
    emit_tool_input(name, &params);
    let result = execute_inner(name, params, ctx).await;
    match &result {
        Ok(v) => emit_tool_output(v),
        Err(e) => emit_tool_error(e),
    }
    result
}

async fn execute_inner(
    name: &str,
    params: Value,
    ctx: &AnomalyToolContext<'_>,
) -> Result<Value, ToolError> {
    match name {
        "list_anomalies" => list_anomalies(params, ctx).await,
        "detect_anomalies" => detect_anomalies(params, ctx).await,
        "explain_anomaly" => explain_anomaly(params, ctx).await,
        _ => Err(ToolError::UnknownTool(name.into())),
    }
}

// ── list_anomalies ──────────────────────────────────────────────────────────

async fn list_anomalies(params: Value, ctx: &AnomalyToolContext<'_>) -> Result<Value, ToolError> {
    let measure = string_param(&params, "measure")?;
    let time_dimension = string_param(&params, "time_dimension")?;
    let granularity = string_param(&params, "granularity")?;
    let period_start = string_param(&params, "period_start")?;
    let period_end = string_param(&params, "period_end")?;

    let filter = AnomalyFilter {
        measure: Some(measure),
        time_dimension: Some(time_dimension),
        granularity: Some(granularity),
        period_start_gte: Some(period_start),
        period_end_lte: Some(period_end),
    };

    let anomalies = ctx
        .store
        .list(ctx.workspace_id, filter)
        .await
        .map_err(|e| match e {
            // A bound the store cannot read is the caller's to fix; the
            // message names the accepted formats.
            AnomalyStoreError::InvalidFilter(msg) => ToolError::BadParams(msg),
            e => ToolError::Execution(e.to_string()),
        })?;

    Ok(json!({
        "count": anomalies.len(),
        "anomalies": anomalies.iter().map(record_to_json).collect::<Vec<_>>(),
    }))
}

// ── detect_anomalies ────────────────────────────────────────────────────────

async fn detect_anomalies(params: Value, ctx: &AnomalyToolContext<'_>) -> Result<Value, ToolError> {
    let measure = string_param(&params, "measure")?;
    let time_dimension = string_param(&params, "time_dimension")?;
    let granularity = string_param(&params, "granularity")?;
    let period_start = string_param(&params, "period_start")?;
    let period_end = string_param(&params, "period_end")?;

    // The detector scores every bucket of the asked period against the
    // history before it, so fetch that history too.
    let fetch_start =
        history_start(&period_start, &granularity).unwrap_or_else(|| period_start.clone());
    let observations = ctx
        .runner
        .run_time_series(
            measure.clone(),
            time_dimension.clone(),
            granularity.clone(),
            (fetch_start, period_end.clone()),
            vec![],
            // None = the runner's configured default, which OxyMetricTreeRunner
            // resolves from the workspace's `.monitor.yml` (see Task 5) so
            // chat-driven detection agrees with scheduled scans.
            None,
        )
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    let result = ctx
        .store
        .detect_and_upsert(
            ctx.workspace_id,
            DetectRequest {
                measure,
                time_dimension,
                granularity,
                observations,
                period: (period_start, period_end),
            },
        )
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    let mut out = json!({
        "count": result.anomalies.len(),
        "total_observations": result.total_observations,
        "anomalies": result.anomalies.iter().map(record_to_json).collect::<Vec<_>>(),
    });
    if !result.missing_periods.is_empty() {
        out["missing_periods"] = json!(result.missing_periods);
        out["missing_periods_note"] = json!(MISSING_NOTE);
    }
    if let Some(msg) = result.message {
        out["message"] = json!(msg);
    }
    Ok(out)
}

/// What the model is told about `missing_periods`.
const MISSING_NOTE: &str = "These buckets returned no row at all, so they were not \
    scored. A bucket with no row may mean no activity (for example a day with no orders) \
    or missing data. Report each one; do not call it a drop to zero without checking.";

/// How many buckets of history to fetch before the asked period. Each is a
/// little above the store's floor for that granularity (8 weekly cycles of
/// days, 8 cycles of weeks, 2 years of months, 12 quarters; see
/// `metric_monitoring::gates::min_history_buckets`), so a series with a few
/// missing rows still has enough.
fn history_buckets(granularity: &str) -> Option<(i64, &'static str)> {
    match granularity {
        "day" => Some((90, "day")),
        "week" => Some((40, "week")),
        "month" => Some((30, "month")),
        "quarter" => Some((48, "month")),
        _ => None,
    }
}

/// The first day to fetch: `period_start` minus [`history_buckets`]. `None`
/// when the start does not parse or the granularity is unknown; the caller
/// then fetches the period alone, as before.
fn history_start(period_start: &str, granularity: &str) -> Option<String> {
    use chrono::{Duration, Months, NaiveDate};
    let start = NaiveDate::parse_from_str(period_start.get(..10)?, "%Y-%m-%d").ok()?;
    let (n, unit) = history_buckets(granularity)?;
    let from = match unit {
        "day" => start - Duration::days(n),
        "week" => start - Duration::weeks(n),
        _ => start.checked_sub_months(Months::new(u32::try_from(n).ok()?))?,
    };
    Some(from.format("%Y-%m-%d").to_string())
}

// ── explain_anomaly ─────────────────────────────────────────────────────────

async fn explain_anomaly(params: Value, ctx: &AnomalyToolContext<'_>) -> Result<Value, ToolError> {
    let id_str = string_param(&params, "anomaly_id")?;
    let id = id_str
        .parse::<Uuid>()
        .map_err(|_| ToolError::BadParams(format!("invalid anomaly_id: {id_str}")))?;

    // Return cached result if available.
    if let Some(cached) = ctx
        .store
        .get_explain_cache(id, ctx.workspace_id)
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?
    {
        return Ok(json!({ "cached": true, "explanation": cached }));
    }

    let record = ctx
        .store
        .get(id, ctx.workspace_id)
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?
        .ok_or_else(|| ToolError::Execution(format!("anomaly {id} not found")))?;

    let current_period = (record.period_start.clone(), record.period_end.clone());
    let previous_period = previous_period_for(&record)
        .ok_or_else(|| ToolError::Execution("could not compute comparison period".to_string()))?;

    // Scope the explain to the anomaly's segment (e.g. the specific restaurant
    // a `group_by` monitor flagged) so the totals and decomposition reflect
    // that segment, not the chain-wide aggregate.
    let filters = crate::anomaly_store::segment_query_filters(&record.filters);

    let explain_result = ctx
        .runner
        .run_explain(
            record.measure.clone(),
            record.time_dimension.clone(),
            current_period,
            previous_period,
            filters,
            Default::default(),
        )
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    let json_result =
        serde_json::to_value(&explain_result).map_err(|e| ToolError::Execution(e.to_string()))?;

    ctx.store
        .set_explain_cache(id, ctx.workspace_id, json_result.clone())
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    Ok(json!({ "cached": false, "explanation": json_result }))
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn string_param(params: &Value, key: &str) -> Result<String, ToolError> {
    params[key]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| ToolError::BadParams(format!("missing '{key}'")))
}

fn record_to_json(r: &AnomalyRecord) -> Value {
    json!({
        "id": r.id,
        "measure": r.measure,
        "time_dimension": r.time_dimension,
        "granularity": r.granularity,
        "period_start": r.period_start,
        "period_end": r.period_end,
        "observed": r.observed,
        "expected": r.expected,
        "lower": r.lower,
        "upper": r.upper,
        "z_score": r.z_score,
        "severity": r.severity,
        "status": r.status,
        "seasonal_period": r.seasonal_period,
    })
}

/// Comparison window for `record`'s root-cause explain: the same-phase bucket
/// one seasonal cycle back, derived from the monitor's persisted seasonality
/// (falling back to the granularity default). Shifting by the seasonal period
/// — not one adjacent bucket — is what keeps a daily anomaly compared against
/// the same weekday rather than the weekend.
fn previous_period_for(record: &AnomalyRecord) -> Option<(String, String)> {
    use chrono::{DateTime, FixedOffset};
    let s: DateTime<FixedOffset> = record.period_start.parse().ok()?;
    let e: DateTime<FixedOffset> = record.period_end.parse().ok()?;
    let (prev_start, prev_end) = crate::anomaly_period::previous_seasonal_range(
        s,
        e,
        &record.granularity,
        record.seasonal_period,
    );
    Some((prev_start.to_rfc3339(), prev_end.to_rfc3339()))
}

#[cfg(test)]
mod history_tests {
    use super::history_start;

    #[test]
    fn the_history_before_the_period_is_fetched() {
        assert_eq!(
            history_start("2026-06-01", "day").as_deref(),
            Some("2026-03-03")
        );
        assert_eq!(
            history_start("2026-06-01", "week").as_deref(),
            Some("2025-08-25")
        );
        assert_eq!(
            history_start("2026-06-01", "month").as_deref(),
            Some("2023-12-01")
        );
        assert_eq!(
            history_start("2026-07-01", "quarter").as_deref(),
            Some("2022-07-01")
        );
        assert_eq!(
            history_start("2026-06-01T00:00:00Z", "day").as_deref(),
            Some("2026-03-03")
        );
    }

    #[test]
    fn an_unreadable_start_or_granularity_fetches_the_period_alone() {
        assert_eq!(history_start("June 2026", "day"), None);
        assert_eq!(history_start("2026-06-01", "hour"), None);
    }
}
