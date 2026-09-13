//! Metrics Analytics API
//!
//! Provides endpoints for querying metric usage analytics data
//! backed by DuckDB storage.

use axum::{
    Router,
    extract::{Json, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use oxy::metrics::{
    ContextTypeBreakdown, MetricAnalytics, MetricAnalyticsResponse, MetricDetailResponse,
    MetricsListResponse, RecentUsage, RelatedMetric, SourceTypeBreakdown, UsageTrendPoint,
};
use serde::Deserialize;
use utoipa::IntoParams;
use uuid::Uuid;

use crate::server::router::AppState;

/// Custom error type for metrics endpoints
#[derive(Debug)]
pub enum MetricsError {
    QueryFailed(String),
    NotFound(String),
}

impl IntoResponse for MetricsError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            MetricsError::QueryFailed(err) => {
                tracing::error!("Metrics query failed: {}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "Failed to query metrics")
            }
            MetricsError::NotFound(metric) => {
                tracing::warn!("Metric not found: {}", metric);
                (StatusCode::NOT_FOUND, "Metric not found")
            }
        };

        (status, message).into_response()
    }
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct MetricsAnalyticsQuery {
    /// Number of days to look back (default: 30)
    #[serde(default = "default_days")]
    pub days: u32,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct MetricsListQuery {
    /// Number of days to look back (default: 30)
    #[serde(default = "default_days")]
    pub days: u32,
    /// Maximum number of metrics to return (default: 20)
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// Offset for pagination (default: 0)
    #[serde(default)]
    pub offset: usize,
}

fn default_days() -> u32 {
    30
}

fn default_limit() -> usize {
    20
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct MetricDetailQuery {
    /// Number of days to look back (default: 30)
    #[serde(default = "default_days")]
    pub days: u32,
}

/// Get metric usage analytics summary
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/metrics/analytics",
    params(MetricsAnalyticsQuery),
    responses(
        (status = 200, description = "Metric analytics data", body = MetricAnalyticsResponse),
        (status = 500, description = "Query failed")
    )
)]
pub async fn get_analytics(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<MetricsAnalyticsQuery>,
) -> Result<Json<MetricAnalyticsResponse>, MetricsError> {
    let storage = state
        .observability()
        .ok_or_else(|| MetricsError::QueryFailed("Observability not configured".into()))?;

    let data = storage
        .get_metrics_analytics(params.days)
        .await
        .map_err(|e| MetricsError::QueryFailed(e.to_string()))?;

    Ok(Json(MetricAnalyticsResponse {
        total_queries: data.total_queries,
        unique_metrics: data.unique_metrics,
        avg_per_metric: data.avg_per_metric,
        most_popular: data.most_popular,
        most_popular_count: data.most_popular_count,
        trend_vs_last_period: data.trend_vs_last_period,
        by_source_type: SourceTypeBreakdown {
            agent: data.by_source_type.agent,
            workflow: data.by_source_type.workflow,
            task: data.by_source_type.task,
            analytics: data.by_source_type.analytics,
        },
        by_context_type: ContextTypeBreakdown {
            sql: data.by_context_type.sql,
            semantic_query: data.by_context_type.semantic_query,
            question: data.by_context_type.question,
            response: data.by_context_type.response,
        },
    }))
}

/// Get paginated list of metrics
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/metrics/list",
    params(MetricsListQuery),
    responses(
        (status = 200, description = "Paginated metrics list", body = MetricsListResponse),
        (status = 500, description = "Query failed")
    )
)]
pub async fn get_metrics_list(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Query(params): Query<MetricsListQuery>,
) -> Result<Json<MetricsListResponse>, MetricsError> {
    let storage = state
        .observability()
        .ok_or_else(|| MetricsError::QueryFailed("Observability not configured".into()))?;

    let data = storage
        .get_metrics_list(params.days, params.limit, params.offset)
        .await
        .map_err(|e| MetricsError::QueryFailed(e.to_string()))?;

    let metrics = data
        .metrics
        .into_iter()
        .map(|m| MetricAnalytics {
            name: m.name,
            count: m.count,
            last_used: Some(m.last_used),
            trend: None,
        })
        .collect();

    Ok(Json(MetricsListResponse {
        metrics,
        total: data.total,
        limit: data.limit,
        offset: data.offset,
    }))
}

/// Get detailed information about a specific metric
#[utoipa::path(
    get,
    path = "/api/{workspace_id}/metrics/{metric_name}",
    params(MetricDetailQuery),
    responses(
        (status = 200, description = "Metric detail data", body = MetricDetailResponse),
        (status = 404, description = "Metric not found"),
        (status = 500, description = "Query failed")
    )
)]
pub async fn get_metric_detail(
    State(state): State<AppState>,
    Path((_workspace_id, metric_name)): Path<(Uuid, String)>,
    Query(params): Query<MetricDetailQuery>,
) -> Result<Json<MetricDetailResponse>, MetricsError> {
    let storage = state
        .observability()
        .ok_or_else(|| MetricsError::QueryFailed("Observability not configured".into()))?;

    let data = storage
        .get_metric_detail(&metric_name, params.days)
        .await
        .map_err(|e| MetricsError::QueryFailed(e.to_string()))?;

    // Check if metric was found (has any usage)
    if data.total_queries == 0 {
        return Err(MetricsError::NotFound(metric_name));
    }

    let usage_trend = data
        .usage_trend
        .into_iter()
        .map(|t| UsageTrendPoint {
            date: t.date,
            count: t.count,
        })
        .collect();

    let related_metrics = data
        .related_metrics
        .into_iter()
        .map(|r| RelatedMetric {
            name: r.name,
            co_occurrence_count: r.co_occurrence_count,
        })
        .collect();

    let recent_usage = data
        .recent_usage
        .into_iter()
        .map(|r| {
            let context_types: Vec<String> =
                serde_json::from_str(&r.context_types).unwrap_or_default();
            RecentUsage {
                source_type: r.source_type,
                source_ref: r.source_ref,
                context_types,
                context: if r.context.is_empty() {
                    None
                } else {
                    Some(r.context)
                },
                trace_id: r.trace_id,
                created_at: r.created_at,
            }
        })
        .collect();

    Ok(Json(MetricDetailResponse {
        name: data.name,
        total_queries: data.total_queries,
        trend_vs_last_period: data.trend_vs_last_period,
        via_agent: data.via_agent,
        via_workflow: data.via_workflow,
        usage_trend,
        related_metrics,
        recent_usage,
    }))
}

pub fn metrics_routes() -> Router<AppState> {
    Router::new()
        .route("/analytics", get(get_analytics))
        .route("/list", get(get_metrics_list))
        .route("/{metric_name}", get(get_metric_detail))
}
