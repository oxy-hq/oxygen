use agentic_core::tools::ToolError;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::anomaly_store::{AnomalyFilter, AnomalyRecord, AnomalyStore, AnomalyStoreError};
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

    let observations = ctx
        .runner
        .run_time_series(
            measure.clone(),
            time_dimension.clone(),
            granularity.clone(),
            (period_start, period_end),
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
            &measure,
            &time_dimension,
            &granularity,
            observations,
        )
        .await
        .map_err(|e| ToolError::Execution(e.to_string()))?;

    let mut out = json!({
        "count": result.anomalies.len(),
        "total_observations": result.total_observations,
        "anomalies": result.anomalies.iter().map(record_to_json).collect::<Vec<_>>(),
    });
    if let Some(msg) = result.message {
        out["message"] = json!(msg);
    }
    Ok(out)
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
