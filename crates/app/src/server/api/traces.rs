use axum::{
    Router,
    extract::{self, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use crate::server::router::AppState;

/// Custom error type for trace endpoints
#[derive(Debug)]
pub enum TracesError {
    QueryFailed(String),
    NotFound(String),
}

impl IntoResponse for TracesError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            TracesError::QueryFailed(err) => {
                tracing::error!("Traces query failed: {}", err);
                (StatusCode::INTERNAL_SERVER_ERROR, "Failed to query traces")
            }
            TracesError::NotFound(trace_id) => {
                tracing::warn!("Trace not found: {}", trace_id);
                (StatusCode::NOT_FOUND, "Trace not found")
            }
        };

        (status, message).into_response()
    }
}

#[derive(Debug, Serialize, Deserialize, IntoParams)]
pub struct TraceListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
    pub agent_ref: Option<String>,
    pub status: Option<String>,
    /// Duration filter: 1h, 24h, 7d, 30d, or all
    pub duration: Option<String>,
    /// Free-text search: trace id, span name, agent ref, or prompt (Theme 3).
    pub search: Option<String>,
    /// Absolute range start (epoch seconds); with `to`, overrides `duration`.
    pub from: Option<i64>,
    /// Absolute range end (epoch seconds).
    pub to: Option<i64>,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TraceResponse {
    #[serde(rename = "traceId")]
    pub trace_id: String,
    #[serde(rename = "spanId")]
    pub span_id: String,
    pub timestamp: String,
    #[serde(rename = "spanName")]
    pub span_name: String,
    #[serde(rename = "serviceName")]
    pub service_name: String,
    #[serde(rename = "durationNs")]
    pub duration_ns: i64,
    #[serde(rename = "statusCode")]
    pub status_code: String,
    #[serde(rename = "statusMessage")]
    pub status_message: String,
    #[serde(rename = "spanKind")]
    pub span_kind: String,
    #[serde(rename = "spanAttributes")]
    pub span_attributes: Vec<(String, String)>,
    #[serde(rename = "eventsAttributes")]
    pub events_attributes: Vec<Vec<(String, String)>>,
    #[serde(rename = "promptTokens")]
    pub prompt_tokens: i64,
    #[serde(rename = "completionTokens")]
    pub completion_tokens: i64,
    #[serde(rename = "totalTokens")]
    pub total_tokens: i64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PaginatedTraceResponse {
    pub items: Vec<TraceResponse>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Full trace span detail response (all columns from spans table)
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TraceDetailSpan {
    #[serde(rename = "timestamp")]
    pub timestamp: String,
    #[serde(rename = "traceId")]
    pub trace_id: String,
    #[serde(rename = "spanId")]
    pub span_id: String,
    #[serde(rename = "parentSpanId")]
    pub parent_span_id: String,
    #[serde(rename = "spanName")]
    pub span_name: String,
    #[serde(rename = "spanKind")]
    pub span_kind: String,
    #[serde(rename = "serviceName")]
    pub service_name: String,
    #[serde(rename = "spanAttributes")]
    pub span_attributes: Vec<(String, String)>,
    #[serde(rename = "duration")]
    pub duration: i64,
    #[serde(rename = "statusCode")]
    pub status_code: String,
    #[serde(rename = "statusMessage")]
    pub status_message: String,
    #[serde(rename = "eventsName")]
    pub events_name: Vec<String>,
    #[serde(rename = "eventsAttributes")]
    pub events_attributes: Vec<Vec<(String, String)>>,
}

// ── JSON helpers ─────────────────────────────────────────────────────────

/// Parse a JSON object string into a flat list of key-value pairs.
fn parse_json_to_pairs(json: &str) -> Vec<(String, String)> {
    serde_json::from_str::<HashMap<String, serde_json::Value>>(json)
        .unwrap_or_default()
        .into_iter()
        .map(|(k, v)| {
            (
                k,
                v.as_str()
                    .map(String::from)
                    .unwrap_or_else(|| v.to_string()),
            )
        })
        .collect()
}

/// Parse event_data JSON array into the nested pairs format expected by the
/// frontend: `Vec<Vec<(String, String)>>` where each inner vec represents one
/// event with its name + attributes flattened into pairs.
fn parse_event_data_to_pairs(json: &str) -> Vec<Vec<(String, String)>> {
    let events: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    events
        .into_iter()
        .map(|ev| {
            let mut pairs = Vec::new();
            if let Some(name) = ev.get("name").and_then(|n| n.as_str()) {
                pairs.push(("name".to_string(), name.to_string()));
            }
            if let Some(attrs) = ev.get("attributes").and_then(|a| a.as_object()) {
                for (k, v) in attrs {
                    pairs.push((
                        k.clone(),
                        v.as_str()
                            .map(String::from)
                            .unwrap_or_else(|| v.to_string()),
                    ));
                }
            }
            pairs
        })
        .collect()
}

fn extract_event_names(json: &str) -> Vec<String> {
    let events: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    events
        .iter()
        .filter_map(|ev| ev.get("name").and_then(|n| n.as_str()).map(String::from))
        .collect()
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// List recent traces
#[utoipa::path(
    get,
    path = "/api/traces",
    responses(
        (status = 200, description = "List of traces", body = PaginatedTraceResponse)
    ),
    params(TraceListQuery)
)]
pub async fn list_traces(
    State(state): State<AppState>,
    Query(params): Query<TraceListQuery>,
) -> Result<extract::Json<PaginatedTraceResponse>, TracesError> {
    let storage = state
        .observability()
        .ok_or_else(|| TracesError::QueryFailed("Observability not configured".into()))?;

    let (traces, total) = storage
        .search_traces(
            params.limit,
            params.offset,
            params.agent_ref.as_deref(),
            params.status.as_deref(),
            params.duration.as_deref(),
            params.search.as_deref(),
            params.from,
            params.to,
        )
        .await
        .map_err(|e| TracesError::QueryFailed(e.to_string()))?;

    let items = traces
        .into_iter()
        .map(|t| TraceResponse {
            trace_id: t.trace_id,
            span_id: t.span_id,
            timestamp: t.timestamp,
            span_name: t.span_name,
            service_name: t.service_name,
            duration_ns: t.duration_ns,
            status_code: t.status_code,
            status_message: t.status_message,
            span_kind: "INTERNAL".to_string(),
            span_attributes: parse_json_to_pairs(&t.span_attributes),
            events_attributes: parse_event_data_to_pairs(&t.event_data),
            prompt_tokens: t.prompt_tokens,
            completion_tokens: t.completion_tokens,
            total_tokens: t.total_tokens,
        })
        .collect();

    Ok(extract::Json(PaginatedTraceResponse {
        items,
        total,
        limit: params.limit,
        offset: params.offset,
    }))
}

/// Get trace detail by TraceId
#[utoipa::path(
    get,
    path = "/api/traces/{trace_id}",
    responses(
        (status = 200, description = "Trace detail with all spans", body = Vec<TraceDetailSpan>),
        (status = 404, description = "Trace not found")
    ),
    params(
        ("trace_id" = String, Path, description = "The trace ID to retrieve")
    )
)]
pub async fn get_trace_detail(
    State(state): State<AppState>,
    Path((_workspace_id, trace_id)): Path<(Uuid, String)>,
) -> Result<extract::Json<Vec<TraceDetailSpan>>, TracesError> {
    let storage = state
        .observability()
        .ok_or_else(|| TracesError::QueryFailed("Observability not configured".into()))?;

    let rows = storage
        .get_trace_detail(&trace_id)
        .await
        .map_err(|e| TracesError::QueryFailed(e.to_string()))?;

    if rows.is_empty() {
        return Err(TracesError::NotFound(trace_id));
    }

    let spans = rows
        .into_iter()
        .map(|r| TraceDetailSpan {
            timestamp: r.timestamp,
            trace_id: r.trace_id,
            span_id: r.span_id,
            parent_span_id: r.parent_span_id,
            span_name: r.span_name,
            span_kind: "INTERNAL".to_string(),
            service_name: r.service_name,
            span_attributes: parse_json_to_pairs(&r.span_attributes),
            duration: r.duration_ns,
            status_code: r.status_code,
            status_message: r.status_message,
            events_name: extract_event_names(&r.event_data),
            events_attributes: parse_event_data_to_pairs(&r.event_data),
        })
        .collect();

    Ok(extract::Json(spans))
}

// Cluster Map API - 2D visualization of intent clusters

#[derive(Debug, Serialize, ToSchema)]
pub struct ClusterMapPoint {
    #[serde(rename = "traceId")]
    pub trace_id: String,
    /// The question/prompt text
    pub question: String,
    /// X coordinate (2D projection)
    pub x: f32,
    /// Y coordinate (2D projection)
    pub y: f32,
    /// Cluster ID (-1 for outliers)
    #[serde(rename = "clusterId")]
    pub cluster_id: i32,
    /// Intent name (or "outlier" for unclustered)
    #[serde(rename = "intentName")]
    pub intent_name: String,
    /// Classification confidence
    pub confidence: f32,
    /// Timestamp of the trace
    pub timestamp: String,
    /// Duration in milliseconds
    #[serde(rename = "durationMs")]
    pub duration_ms: Option<f64>,
    /// Status of the trace (ok, error, unset)
    pub status: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ClusterSummary {
    #[serde(rename = "clusterId")]
    pub cluster_id: i32,
    #[serde(rename = "intentName")]
    pub intent_name: String,
    /// Intent description
    pub description: String,
    /// Number of points in this cluster
    pub count: usize,
    /// Color for this cluster (hex)
    pub color: String,
    #[serde(rename = "sampleQuestions")]
    pub sample_questions: Vec<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ClusterMapResponse {
    /// All points in the visualization
    pub points: Vec<ClusterMapPoint>,
    pub clusters: Vec<ClusterSummary>,
    #[serde(rename = "totalPoints")]
    pub total_points: usize,
    /// Number of outliers
    #[serde(rename = "outlierCount")]
    pub outlier_count: usize,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ClusterMapQuery {
    /// Maximum number of points to return
    #[serde(default = "default_cluster_map_limit")]
    pub limit: usize,
    /// Number of days to look back
    #[serde(default = "default_cluster_map_days")]
    pub days: u32,
    /// Filter by source (e.g., agent ref)
    pub source: Option<String>,
}

fn default_cluster_map_limit() -> usize {
    500
}

fn default_cluster_map_days() -> u32 {
    30
}

/// Get cluster map data for visualization
#[utoipa::path(
    get,
    path = "/clusters/map",
    params(ClusterMapQuery),
    responses(
        (status = 200, description = "Cluster map data", body = ClusterMapResponse),
        (status = 503, description = "Storage unavailable")
    )
)]
pub async fn get_cluster_map(
    State(state): State<AppState>,
    Query(query): Query<ClusterMapQuery>,
) -> Result<extract::Json<ClusterMapResponse>, TracesError> {
    let storage = state
        .observability()
        .ok_or_else(|| TracesError::QueryFailed("Observability not configured".into()))?;

    let embeddings = storage
        .get_cluster_map_data(query.days, query.limit, query.source.as_deref())
        .await
        .map_err(|e| TracesError::QueryFailed(e.to_string()))?;

    if embeddings.is_empty() {
        return Ok(extract::Json(ClusterMapResponse {
            points: vec![],
            clusters: vec![],
            total_points: 0,
            outlier_count: 0,
        }));
    }

    let cluster_infos = storage.get_cluster_infos().await.unwrap_or_default();

    // Project embeddings to 2D using simple PCA-like approach
    let points_2d = project_to_2d(&embeddings);

    let cluster_colors = generate_cluster_colors(cluster_infos.len() + 1); // +1 for outliers
    let mut cluster_color_map: HashMap<i32, String> = HashMap::new();
    cluster_color_map.insert(-1, cluster_colors[0].clone()); // Outliers get first color
    for (i, c) in cluster_infos.iter().enumerate() {
        cluster_color_map.insert(
            c.cluster_id,
            cluster_colors
                .get(i + 1)
                .cloned()
                .unwrap_or_else(|| "#6b7280".to_string()),
        );
    }

    // Collect trace IDs for enrichment
    let trace_ids: Vec<String> = embeddings.iter().map(|e| e.trace_id.clone()).collect();

    // Fetch trace status and duration from spans table
    let enrichments = storage
        .get_trace_enrichments(&trace_ids)
        .await
        .unwrap_or_default();

    let enrichment_map: HashMap<String, _> = enrichments
        .into_iter()
        .map(|e| (e.trace_id.clone(), e))
        .collect();

    let mut points: Vec<ClusterMapPoint> = Vec::with_capacity(embeddings.len());
    let mut cluster_counts: HashMap<i32, usize> = HashMap::new();

    for (i, emb) in embeddings.iter().enumerate() {
        let (x, y) = points_2d[i];
        let cluster_id = if emb.intent_name == "unknown" {
            -1
        } else {
            emb.cluster_id
        };

        *cluster_counts.entry(cluster_id).or_insert(0) += 1;

        // Get enrichment data if available
        let enrichment = enrichment_map.get(&emb.trace_id);
        let (duration_ms, status) = match enrichment {
            Some(e) => {
                let duration_ms = Some(e.duration_ns as f64 / 1_000_000.0);
                let status = match e.status_code.as_str() {
                    "STATUS_CODE_OK" | "OK" => Some("ok".to_string()),
                    "STATUS_CODE_ERROR" | "ERROR" => Some("error".to_string()),
                    _ => Some("ok".to_string()),
                };
                (duration_ms, status)
            }
            None => (None, None),
        };

        points.push(ClusterMapPoint {
            trace_id: emb.trace_id.clone(),
            question: emb.question.clone(),
            x,
            y,
            cluster_id,
            intent_name: emb.intent_name.clone(),
            confidence: emb.confidence,
            timestamp: emb.classified_at.clone(),
            duration_ms,
            status,
        });
    }

    let parse_sample_questions =
        |sq: &str| -> Vec<String> { serde_json::from_str(sq).unwrap_or_default() };

    let mut clusters: Vec<ClusterSummary> = cluster_infos
        .iter()
        .map(|c| ClusterSummary {
            cluster_id: c.cluster_id,
            intent_name: c.intent_name.clone(),
            description: c.intent_description.clone(),
            count: *cluster_counts.get(&c.cluster_id).unwrap_or(&0),
            color: cluster_color_map
                .get(&c.cluster_id)
                .cloned()
                .unwrap_or_else(|| "#6b7280".to_string()),
            sample_questions: parse_sample_questions(&c.sample_questions),
        })
        .filter(|c| c.count > 0)
        .collect();

    // Add outlier "cluster" if there are any
    let outlier_count = *cluster_counts.get(&-1).unwrap_or(&0);
    if outlier_count > 0 {
        clusters.insert(
            0,
            ClusterSummary {
                cluster_id: -1,
                intent_name: "Outliers".to_string(),
                description: "Questions that don't fit any cluster".to_string(),
                count: outlier_count,
                color: cluster_color_map
                    .get(&-1)
                    .cloned()
                    .unwrap_or_else(|| "#6b7280".to_string()),
                sample_questions: vec![],
            },
        );
    }

    let total_points = points.len();

    Ok(extract::Json(ClusterMapResponse {
        points,
        clusters,
        total_points,
        outlier_count,
    }))
}

// ── PCA projection and color helpers (pure computation) ──────────────────

use oxy_observability::types::ClusterMapDataRow;

/// Fast 2D projection using PCA (Principal Component Analysis)
fn project_to_2d(embeddings: &[ClusterMapDataRow]) -> Vec<(f32, f32)> {
    use linfa::prelude::*;
    use linfa_reduction::Pca;
    use ndarray::{Array2, Axis};

    if embeddings.is_empty() {
        return vec![];
    }

    let n = embeddings.len();
    let dims = embeddings[0].embedding.len();

    if dims == 0 {
        return vec![(0.0, 0.0); n];
    }

    if n < 2 || dims < 2 {
        return vec![(0.0, 0.0); n];
    }

    // Convert embeddings to ndarray matrix (n_samples x n_features)
    let mut data = Array2::<f64>::zeros((n, dims));
    for (i, emb) in embeddings.iter().enumerate() {
        for (j, &val) in emb.embedding.iter().enumerate() {
            data[[i, j]] = val as f64;
        }
    }

    let dataset = DatasetBase::from(data);

    let pca = match Pca::params(2).fit(&dataset) {
        Ok(pca) => pca,
        Err(e) => {
            tracing::warn!("PCA fitting failed: {}", e);
            return vec![(0.0, 0.0); n];
        }
    };

    let projected = pca.transform(dataset);

    let ncols = projected.records().ncols();
    let mut positions: Vec<(f64, f64)> = projected
        .records()
        .axis_iter(Axis(0))
        .map(|row| {
            let x = if ncols > 0 { row[0] } else { 0.0 };
            let y = if ncols > 1 { row[1] } else { 0.0 };
            (x, y)
        })
        .collect();

    normalize_positions(&mut positions);

    positions
        .iter()
        .map(|(x, y)| (*x as f32, *y as f32))
        .collect()
}

/// Normalize positions to display range [-400, 400] x [-300, 300]
fn normalize_positions(positions: &mut [(f64, f64)]) {
    if positions.is_empty() {
        return;
    }

    let (min_x, max_x, min_y, max_y) = positions.iter().fold(
        (f64::MAX, f64::MIN, f64::MAX, f64::MIN),
        |(min_x, max_x, min_y, max_y), (x, y)| {
            (min_x.min(*x), max_x.max(*x), min_y.min(*y), max_y.max(*y))
        },
    );

    let range_x = (max_x - min_x).max(0.001);
    let range_y = (max_y - min_y).max(0.001);

    for (x, y) in positions.iter_mut() {
        *x = (*x - min_x) / range_x * 800.0 - 400.0;
        *y = (*y - min_y) / range_y * 600.0 - 300.0;
    }
}

fn generate_cluster_colors(n: usize) -> Vec<String> {
    let palette = [
        "#9ca3af", // Gray for outliers
        "#3b82f6", // Blue
        "#ef4444", // Red
        "#22c55e", // Green
        "#f59e0b", // Amber
        "#8b5cf6", // Purple
        "#06b6d4", // Cyan
        "#ec4899", // Pink
        "#f97316", // Orange
        "#84cc16", // Lime
        "#14b8a6", // Teal
        "#6366f1", // Indigo
        "#a855f7", // Violet
        "#0ea5e9", // Sky
        "#10b981", // Emerald
        "#eab308", // Yellow
    ];

    (0..n)
        .map(|i| palette[i % palette.len()].to_string())
        .collect()
}

pub fn traces_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_traces))
        .route("/{trace_id}", get(get_trace_detail))
        .route("/clusters/map", get(get_cluster_map))
}
