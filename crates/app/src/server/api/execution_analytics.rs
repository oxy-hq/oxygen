//! Execution Analytics API
//!
//! Provides endpoints for querying execution analytics data,
//! tracking verified vs generated executions across different tool types.
//! Backed by DuckDB storage.

use axum::{
    Router,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use oxy::execution_analytics::{
    AgentExecutionStats, ExecutionCostResponse, ExecutionDetail, ExecutionListResponse,
    ExecutionSummary, ExecutionTimeBucket, HistogramBucket, LatencyHistogramResponse,
    LatencyPercentilePoint, LatencyPercentilesResponse, LatencyTriple, ModelCost, model_cost_usd,
};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::server::router::AppState;

#[derive(Debug)]
pub enum ExecutionAnalyticsError {
    QueryFailed(String),
}

impl IntoResponse for ExecutionAnalyticsError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ExecutionAnalyticsError::QueryFailed(err) => {
                tracing::error!("Execution analytics query failed: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to query execution analytics",
                )
            }
        };

        (status, message).into_response()
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct SummaryQuery {
    /// Number of days to look back (default: 7)
    #[serde(default = "default_days")]
    pub days: u32,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct TimeSeriesQuery {
    /// Number of days to look back (default: 7)
    #[serde(default = "default_days")]
    pub days: u32,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct AgentStatsQuery {
    /// Number of days to look back (default: 7)
    #[serde(default = "default_days")]
    pub days: u32,
    /// Maximum number of agents to return (default: 10)
    #[serde(default = "default_agent_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ExecutionsQuery {
    /// Number of days to look back (default: 7)
    #[serde(default = "default_days")]
    pub days: u32,
    /// Maximum number of executions to return (default: 50)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Offset for pagination (default: 0)
    #[serde(default)]
    pub offset: usize,
    /// Filter by execution type (semantic_query, omni_query, sql_generated, workflow, agent_tool)
    pub execution_type: Option<String>,
    /// Filter by verified status
    pub is_verified: Option<bool>,
    /// Filter by source reference (agent or workflow ref)
    pub source_ref: Option<String>,
    /// Filter by status (success, error)
    pub status: Option<String>,
}

/// Shared look-back query for the latency/cost endpoints.
#[derive(Debug, Deserialize, IntoParams)]
pub struct DaysQuery {
    /// Number of days to look back (default: 7)
    #[serde(default = "default_days")]
    pub days: u32,
}

fn default_days() -> u32 {
    7
}

fn default_limit() -> usize {
    50
}

fn default_agent_limit() -> usize {
    10
}

/// Get execution analytics summary
///
/// Returns aggregated statistics about verified vs generated executions
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/execution-analytics/summary",
    params(SummaryQuery),
    responses(
        (status = 200, description = "Execution analytics summary", body = ExecutionSummary),
        (status = 500, description = "Query failed")
    )
)]
pub async fn get_summary(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<SummaryQuery>,
) -> Result<Json<ExecutionSummary>, ExecutionAnalyticsError> {
    let storage = state.observability().ok_or_else(|| {
        ExecutionAnalyticsError::QueryFailed("Observability not configured".into())
    })?;

    let data = storage
        .get_execution_summary(params.days)
        .await
        .map_err(|e| ExecutionAnalyticsError::QueryFailed(e.to_string()))?;

    let total = data.total_executions.max(1) as f64;
    let verified_percent = (data.verified_count as f64 / total) * 100.0;
    let generated_percent = (data.generated_count as f64 / total) * 100.0;

    let success_rate_verified = if data.verified_count > 0 {
        (data.success_count_verified as f64 / data.verified_count as f64) * 100.0
    } else {
        0.0
    };

    let success_rate_generated = if data.generated_count > 0 {
        (data.success_count_generated as f64 / data.generated_count as f64) * 100.0
    } else {
        0.0
    };

    let type_counts = [
        (data.semantic_query_count, "semantic_query"),
        (data.omni_query_count, "omni_query"),
        (data.sql_generated_count, "sql_generated"),
        (data.workflow_count, "workflow"),
        (data.agent_tool_count, "agent_tool"),
    ];
    let most_executed_type = type_counts
        .iter()
        .max_by_key(|(count, _)| *count)
        .map(|(_, name)| *name)
        .unwrap_or("none")
        .to_string();

    Ok(Json(ExecutionSummary {
        total_executions: data.total_executions,
        verified_count: data.verified_count,
        generated_count: data.generated_count,
        verified_percent,
        generated_percent,
        success_rate_verified,
        success_rate_generated,
        most_executed_type,
        semantic_query_count: data.semantic_query_count,
        omni_query_count: data.omni_query_count,
        sql_generated_count: data.sql_generated_count,
        workflow_count: data.workflow_count,
        agent_tool_count: data.agent_tool_count,
    }))
}

/// Get execution analytics time series
///
/// Returns execution counts bucketed by day
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/execution-analytics/time-series",
    params(TimeSeriesQuery),
    responses(
        (status = 200, description = "Execution time series data", body = Vec<ExecutionTimeBucket>),
        (status = 500, description = "Query failed")
    )
)]
pub async fn get_time_series(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<TimeSeriesQuery>,
) -> Result<Json<Vec<ExecutionTimeBucket>>, ExecutionAnalyticsError> {
    let storage = state.observability().ok_or_else(|| {
        ExecutionAnalyticsError::QueryFailed("Observability not configured".into())
    })?;

    let data = storage
        .get_execution_time_series(params.days)
        .await
        .map_err(|e| ExecutionAnalyticsError::QueryFailed(e.to_string()))?;

    let time_series = data
        .into_iter()
        .map(|row| ExecutionTimeBucket {
            timestamp: row.date,
            verified_count: row.verified_count,
            generated_count: row.generated_count,
            semantic_query_count: Some(row.semantic_query_count),
            omni_query_count: Some(row.omni_query_count),
            sql_generated_count: Some(row.sql_generated_count),
            workflow_count: Some(row.workflow_count),
            agent_tool_count: Some(row.agent_tool_count),
        })
        .collect();

    Ok(Json(time_series))
}

/// Get per-agent execution statistics
///
/// Returns execution statistics grouped by agent
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/execution-analytics/agents",
    params(AgentStatsQuery),
    responses(
        (status = 200, description = "Agent execution statistics", body = Vec<AgentExecutionStats>),
        (status = 500, description = "Query failed")
    )
)]
pub async fn get_agent_stats(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<AgentStatsQuery>,
) -> Result<Json<Vec<AgentExecutionStats>>, ExecutionAnalyticsError> {
    let storage = state.observability().ok_or_else(|| {
        ExecutionAnalyticsError::QueryFailed("Observability not configured".into())
    })?;

    let data = storage
        .get_execution_agent_stats(params.days, params.limit)
        .await
        .map_err(|e| ExecutionAnalyticsError::QueryFailed(e.to_string()))?;

    let agent_stats = data
        .into_iter()
        .map(|row| {
            let total = row.total_executions.max(1) as f64;
            let type_counts = [
                (row.semantic_query_count, "semantic_query"),
                (row.omni_query_count, "omni_query"),
                (row.sql_generated_count, "sql_generated"),
                (row.workflow_count, "workflow"),
                (row.agent_tool_count, "agent_tool"),
            ];
            let most_executed_type = type_counts
                .iter()
                .max_by_key(|(count, _)| *count)
                .map(|(_, name)| *name)
                .unwrap_or("none")
                .to_string();

            AgentExecutionStats {
                agent_ref: row.agent_ref,
                total_executions: row.total_executions,
                verified_count: row.verified_count,
                generated_count: row.generated_count,
                verified_percent: (row.verified_count as f64 / total) * 100.0,
                most_executed_type,
                success_rate: (row.success_count as f64 / total) * 100.0,
            }
        })
        .collect();

    Ok(Json(agent_stats))
}

/// Get paginated execution details
///
/// Returns detailed execution records with filtering and pagination
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/execution-analytics/executions",
    params(ExecutionsQuery),
    responses(
        (status = 200, description = "Paginated execution details", body = ExecutionListResponse),
        (status = 500, description = "Query failed")
    )
)]
pub async fn get_executions(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<ExecutionsQuery>,
) -> Result<Json<ExecutionListResponse>, ExecutionAnalyticsError> {
    let storage = state.observability().ok_or_else(|| {
        ExecutionAnalyticsError::QueryFailed("Observability not configured".into())
    })?;

    let data = storage
        .get_execution_list(
            params.days,
            params.limit,
            params.offset,
            params.execution_type.as_deref(),
            params.is_verified,
            params.source_ref.as_deref(),
            params.status.as_deref(),
        )
        .await
        .map_err(|e| ExecutionAnalyticsError::QueryFailed(e.to_string()))?;

    let executions = data
        .executions
        .into_iter()
        .map(|row| ExecutionDetail {
            trace_id: row.trace_id,
            span_id: row.span_id,
            timestamp: row.timestamp,
            execution_type: row.execution_type,
            is_verified: row.is_verified == "true",
            source_type: row.source_type,
            source_ref: row.source_ref,
            status: row.status,
            duration_ms: row.duration_ns as f64 / 1_000_000.0,
            database: non_empty(row.database),
            output: non_empty(row.output),
            error: non_empty(row.error),
            topic: non_empty(row.topic),
            semantic_query_params: non_empty(row.semantic_query_params),
            generated_sql: non_empty(row.generated_sql),
            integration: non_empty(row.integration),
            endpoint: non_empty(row.endpoint),
            sql: non_empty(row.sql),
            sql_ref: non_empty(row.sql_ref),
            user_question: non_empty(row.user_question),
            workflow_ref: non_empty(row.workflow_ref),
            agent_ref: non_empty(row.agent_ref),
            tool_input: non_empty(row.input).or_else(|| non_empty(row.tool_input)),
        })
        .collect();

    Ok(Json(ExecutionListResponse {
        executions,
        total: data.total,
        limit: data.limit,
        offset: data.offset,
    }))
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}

/// Get latency percentiles (p50/p95/p99) — overall window plus a daily series.
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/execution-analytics/percentiles",
    params(DaysQuery),
    responses((status = 200, body = LatencyPercentilesResponse), (status = 500))
)]
pub async fn get_percentiles(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<DaysQuery>,
) -> Result<Json<LatencyPercentilesResponse>, ExecutionAnalyticsError> {
    let storage = state.observability().ok_or_else(|| {
        ExecutionAnalyticsError::QueryFailed("Observability not configured".into())
    })?;

    let data = storage
        .get_latency_percentiles(params.days)
        .await
        .map_err(|e| ExecutionAnalyticsError::QueryFailed(e.to_string()))?;

    Ok(Json(LatencyPercentilesResponse {
        overall: LatencyTriple {
            p50_ms: data.overall.p50_ms,
            p95_ms: data.overall.p95_ms,
            p99_ms: data.overall.p99_ms,
        },
        series: data
            .series
            .into_iter()
            .map(|p| LatencyPercentilePoint {
                date: p.date,
                p50_ms: p.p50_ms,
                p95_ms: p.p95_ms,
                p99_ms: p.p99_ms,
            })
            .collect(),
    }))
}

/// Get the latency histogram (log buckets) with p50/p95/p99 markers.
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/execution-analytics/histogram",
    params(DaysQuery),
    responses((status = 200, body = LatencyHistogramResponse), (status = 500))
)]
pub async fn get_histogram(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<DaysQuery>,
) -> Result<Json<LatencyHistogramResponse>, ExecutionAnalyticsError> {
    let storage = state.observability().ok_or_else(|| {
        ExecutionAnalyticsError::QueryFailed("Observability not configured".into())
    })?;

    let data = storage
        .get_latency_histogram(params.days)
        .await
        .map_err(|e| ExecutionAnalyticsError::QueryFailed(e.to_string()))?;

    Ok(Json(LatencyHistogramResponse {
        buckets: data
            .buckets
            .into_iter()
            .map(|b| HistogramBucket {
                upper_ms: b.upper_ms,
                count: b.count,
            })
            .collect(),
        p50_ms: data.percentiles.p50_ms,
        p95_ms: data.percentiles.p95_ms,
        p99_ms: data.percentiles.p99_ms,
    }))
}

/// Get per-model LLM token usage and estimated cost.
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/execution-analytics/cost",
    params(DaysQuery),
    responses((status = 200, body = ExecutionCostResponse), (status = 500))
)]
pub async fn get_cost(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<DaysQuery>,
) -> Result<Json<ExecutionCostResponse>, ExecutionAnalyticsError> {
    let storage = state.observability().ok_or_else(|| {
        ExecutionAnalyticsError::QueryFailed("Observability not configured".into())
    })?;

    let data = storage
        .get_model_usage(params.days)
        .await
        .map_err(|e| ExecutionAnalyticsError::QueryFailed(e.to_string()))?;

    let mut total_cost_usd = 0.0;
    let mut total_tokens = 0u64;
    let by_model = data
        .into_iter()
        .map(|m| {
            let tokens = m.input_tokens + m.output_tokens;
            let cost_usd = model_cost_usd(&m.model, m.input_tokens, m.output_tokens);
            total_cost_usd += cost_usd;
            total_tokens += tokens;
            ModelCost {
                model: m.model,
                calls: m.calls,
                tokens,
                cost_usd,
                p95_ms: m.p95_ms,
            }
        })
        .collect();

    Ok(Json(ExecutionCostResponse {
        total_cost_usd,
        total_tokens,
        by_model,
    }))
}

pub fn execution_analytics_routes() -> Router<AppState> {
    Router::new()
        .route("/summary", get(get_summary))
        .route("/time-series", get(get_time_series))
        .route("/agents", get(get_agent_stats))
        .route("/executions", get(get_executions))
        .route("/percentiles", get(get_percentiles))
        .route("/histogram", get(get_histogram))
        .route("/cost", get(get_cost))
}
