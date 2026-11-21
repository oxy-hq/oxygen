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

use oxy_app_core::AppState;
use oxy_observability::WorkspaceScope;

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
    Path(workspace_id): Path<Uuid>,
    Query(params): Query<TraceListQuery>,
) -> Result<extract::Json<PaginatedTraceResponse>, TracesError> {
    let storage = state
        .observability()
        .ok_or_else(|| TracesError::QueryFailed("Observability not configured".into()))?;

    let (traces, total) = storage
        .search_traces(
            &WorkspaceScope::of(workspace_id),
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
    Path((workspace_id, trace_id)): Path<(Uuid, String)>,
) -> Result<extract::Json<Vec<TraceDetailSpan>>, TracesError> {
    let storage = state
        .observability()
        .ok_or_else(|| TracesError::QueryFailed("Observability not configured".into()))?;

    let rows = storage
        .get_trace_detail(&WorkspaceScope::of(workspace_id), &trace_id)
        .await
        .map_err(|e| TracesError::QueryFailed(e.to_string()))?;

    // Another workspace's trace id lands here too: it matches no row, so it
    // is not found — the same answer as an id nobody ever had, and no way to
    // tell the two apart from outside.
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

pub fn traces_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(list_traces))
        .route("/{trace_id}", get(get_trace_detail))
}
