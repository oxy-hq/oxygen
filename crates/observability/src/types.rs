//! Shared types for the observability store.
//!
//! These types form the "row" and "record" shapes exchanged across the
//! [`crate::store::ObservabilityStore`] trait. They are intentionally free of
//! any backend-specific types, so the trait's consumers — and its test doubles
//! — never depend on the ClickHouse client crate.

// ── Write records ──────────────────────────────────────────────────────────

/// A single span record to be inserted into the backing store.
#[derive(Debug, Clone)]
pub struct SpanRecord {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub span_name: String,
    pub service_name: String,
    /// JSON object string, e.g. `{"key": "value"}`
    pub span_attributes: String,
    pub duration_ns: i64,
    pub status_code: String,
    pub status_message: String,
    /// JSON array of event objects, e.g. `[{"name":"evt","attributes":{}}]`
    pub event_data: String,
    /// ISO 8601 timestamp string
    pub timestamp: String,
}

#[derive(Debug, Clone)]
pub struct MetricUsageRecord {
    pub metric_name: String,
    pub source_type: String,
    pub source_ref: String,
    pub context: String,
    /// JSON array string, e.g. `["type1", "type2"]`
    pub context_types: String,
    pub trace_id: String,
}

#[derive(Debug, Clone)]
pub struct ClassificationRecord {
    pub trace_id: String,
    pub question: String,
    pub cluster_id: i32,
    pub intent_name: String,
    pub confidence: f32,
    pub embedding: Vec<f32>,
    pub source_type: String,
    pub source: String,
}

/// An intent cluster record (upserted via `INSERT OR REPLACE`).
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    pub cluster_id: i32,
    pub intent_name: String,
    pub intent_description: String,
    pub centroid: Vec<f32>,
    pub sample_questions: String,
    pub question_count: i64,
}

// ── Trace query rows ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TraceRow {
    pub trace_id: String,
    pub span_id: String,
    pub timestamp: String,
    pub span_name: String,
    pub service_name: String,
    pub duration_ns: i64,
    pub status_code: String,
    pub status_message: String,
    pub span_attributes: String,
    pub event_data: String,
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone)]
pub struct TraceDetailRow {
    pub timestamp: String,
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: String,
    pub span_name: String,
    pub service_name: String,
    pub span_attributes: String,
    pub duration_ns: i64,
    pub status_code: String,
    pub status_message: String,
    pub event_data: String,
}

#[derive(Debug, Clone)]
pub struct ClusterMapDataRow {
    pub trace_id: String,
    pub question: String,
    pub embedding: Vec<f32>,
    pub cluster_id: i32,
    pub intent_name: String,
    pub confidence: f32,
    pub classified_at: String,
    pub source: String,
}

#[derive(Debug, Clone)]
pub struct ClusterInfoRow {
    pub cluster_id: i32,
    pub intent_name: String,
    pub intent_description: String,
    pub sample_questions: String,
}

#[derive(Debug, Clone)]
pub struct TraceEnrichmentRow {
    pub trace_id: String,
    pub status_code: String,
    pub duration_ns: i64,
}

// ── Intent analytics rows ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct IntentAnalyticsRow {
    pub intent_name: String,
    pub count: u64,
}

// ── Metric analytics result types ──────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MetricAnalyticsData {
    pub total_queries: u64,
    pub unique_metrics: u64,
    pub avg_per_metric: f64,
    pub most_popular: Option<String>,
    pub most_popular_count: Option<u64>,
    pub trend_vs_last_period: Option<String>,
    pub by_source_type: SourceTypeBreakdownData,
    pub by_context_type: ContextTypeBreakdownData,
}

#[derive(Debug, Clone)]
pub struct SourceTypeBreakdownData {
    pub agent: u64,
    pub workflow: u64,
    pub task: u64,
    pub analytics: u64,
}

#[derive(Debug, Clone)]
pub struct ContextTypeBreakdownData {
    pub sql: u64,
    pub semantic_query: u64,
    pub question: u64,
    pub response: u64,
}

#[derive(Debug, Clone)]
pub struct MetricListItem {
    pub name: String,
    pub count: u64,
    pub last_used: String,
}

#[derive(Debug, Clone)]
pub struct MetricsListData {
    pub metrics: Vec<MetricListItem>,
    pub total: u64,
    pub limit: usize,
    pub offset: usize,
}

#[derive(Debug, Clone)]
pub struct UsageTrendPointData {
    pub date: String,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct RelatedMetricData {
    pub name: String,
    pub co_occurrence_count: u64,
}

#[derive(Debug, Clone)]
pub struct RecentUsageData {
    pub source_type: String,
    pub source_ref: String,
    pub context_types: String,
    pub trace_id: String,
    pub created_at: String,
    pub context: String,
}

#[derive(Debug, Clone)]
pub struct MetricDetailData {
    pub name: String,
    pub total_queries: u64,
    pub trend_vs_last_period: Option<String>,
    pub via_agent: u64,
    pub via_workflow: u64,
    pub usage_trend: Vec<UsageTrendPointData>,
    pub related_metrics: Vec<RelatedMetricData>,
    pub recent_usage: Vec<RecentUsageData>,
}

// ── Execution analytics result types ───────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ExecutionSummaryData {
    pub total_executions: u64,
    pub verified_count: u64,
    pub generated_count: u64,
    pub success_count_verified: u64,
    pub success_count_generated: u64,
    pub semantic_query_count: u64,
    pub omni_query_count: u64,
    pub sql_generated_count: u64,
    pub workflow_count: u64,
    pub agent_tool_count: u64,
}

#[derive(Debug, Clone)]
pub struct ExecutionTimeBucketData {
    pub date: String,
    pub verified_count: u64,
    pub generated_count: u64,
    pub semantic_query_count: u64,
    pub omni_query_count: u64,
    pub sql_generated_count: u64,
    pub workflow_count: u64,
    pub agent_tool_count: u64,
}

#[derive(Debug, Clone)]
pub struct AgentExecutionStatsData {
    pub agent_ref: String,
    pub total_executions: u64,
    pub verified_count: u64,
    pub generated_count: u64,
    pub success_count: u64,
    pub semantic_query_count: u64,
    pub omni_query_count: u64,
    pub sql_generated_count: u64,
    pub workflow_count: u64,
    pub agent_tool_count: u64,
}

#[derive(Debug, Clone)]
pub struct ExecutionDetailData {
    pub trace_id: String,
    pub span_id: String,
    pub timestamp: String,
    pub execution_type: String,
    pub is_verified: String,
    pub source_type: String,
    pub source_ref: String,
    pub status: String,
    pub duration_ns: i64,
    pub database: String,
    pub topic: String,
    pub semantic_query_params: String,
    pub generated_sql: String,
    pub integration: String,
    pub endpoint: String,
    pub sql: String,
    pub sql_ref: String,
    pub user_question: String,
    pub workflow_ref: String,
    pub agent_ref: String,
    pub tool_input: String,
    pub input: String,
    pub output: String,
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct ExecutionListData {
    pub executions: Vec<ExecutionDetailData>,
    pub total: u64,
    pub limit: usize,
    pub offset: usize,
}

/// A p50/p95/p99 latency triple, in milliseconds.
#[derive(Debug, Clone, Default)]
pub struct LatencyPercentiles {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
}

#[derive(Debug, Clone)]
pub struct LatencyPercentilePoint {
    pub date: String,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
}

/// Latency percentiles: overall window plus a daily series.
#[derive(Debug, Clone, Default)]
pub struct LatencyPercentilesData {
    pub overall: LatencyPercentiles,
    pub series: Vec<LatencyPercentilePoint>,
}

/// One latency-histogram bucket (`upper_ms` is the bucket's inclusive upper
/// bound in milliseconds).
#[derive(Debug, Clone)]
pub struct HistogramBucketData {
    pub upper_ms: f64,
    pub count: u64,
}

/// Latency histogram plus the p50/p95/p99 markers to overlay on it.
#[derive(Debug, Clone, Default)]
pub struct LatencyHistogramData {
    pub buckets: Vec<HistogramBucketData>,
    pub percentiles: LatencyPercentiles,
}

/// Per-model LLM token usage. Cost is computed downstream from a price map
/// (tokens are engine-neutral; prices are not the storage layer's concern).
#[derive(Debug, Clone)]
pub struct ModelUsageData {
    pub model: String,
    pub calls: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub p95_ms: f64,
}

// ── Custom-app telemetry ─────────────────────────────────────────────────────

/// `kind` values for [`CustomAppEventRecord`]. One per surface Oxy terminates.
pub mod custom_app_kind {
    /// An HTML shell serve — the request a human's page load is.
    pub const SERVE: &str = "serve";
    /// A bundle asset (JS, CSS, image) served from the build store.
    pub const ASSET: &str = "asset";
    /// An Oxy Function invocation, any mode.
    pub const FUNCTION: &str = "fn";
    /// A data-plane call (`/query`, `/semantic-query`, an agent ask).
    pub const DATA: &str = "data";
    /// A browser-reported event from the injected client runtime.
    pub const CLIENT: &str = "client";
}

/// `outcome` values for [`CustomAppEventRecord`], following the SRE error
/// taxonomy — availability is the ratio of real requests that succeeded, and
/// the three failure classes do not all show up in a status code.
pub mod custom_app_outcome {
    /// Served what was asked for.
    pub const OK: &str = "ok";
    /// **Explicit** failure — a 5xx, a refused connection, a thrown function.
    pub const ERROR: &str = "error";
    /// **Implicit** failure — a 200 carrying the wrong thing. The white-screen
    /// case: the shell was served, the app never mounted. Invisible to a status
    /// code, and the single most common way a custom app is "down".
    pub const BROKEN: &str = "broken";
    /// **Policy** failure — succeeded, but outside the objective.
    pub const SLOW: &str = "slow";
}

/// One wide event per custom-app request. See `CREATE_CUSTOM_APP_EVENTS_TABLE`.
///
/// Ids are `String` rather than `Uuid` because the table stores them as
/// `String` and an absent id must be distinguishable from a nil one — a
/// fabricated `00000000-…` would join rows that have nothing to do with each
/// other.
#[derive(Debug, Clone)]
pub struct CustomAppEventRecord {
    /// Unix milliseconds, stamped where the event happened. Not left to the
    /// flush to fill in: a batch can sit for seconds, and an availability
    /// window computed from flush time attributes an outage to the wrong minute.
    pub timestamp_ms: i64,
    pub org_id: String,
    pub app_id: String,
    pub build_id: String,
    pub request_id: String,
    pub session_id: String,
    pub user_id: String,
    /// One of [`custom_app_kind`].
    pub kind: String,
    pub route: String,
    pub status: u16,
    pub duration_ms: u32,
    /// Host ops an `fn` invocation made — the subrequest count. `0` on every
    /// other kind, which have no host calls to make.
    pub host_calls: u32,
    /// Platform setup milliseconds for an `fn` invocation: OS thread spawn,
    /// isolate creation, bootstrap and script compile. `duration_ms - init_ms`
    /// is the tenant's own time.
    ///
    /// `0` means "not applicable or never reached" — a non-`fn` row, a build
    /// that failed to compile, or a timeout during setup. It does not mean
    /// setup was instant.
    pub init_ms: u32,
    pub bytes: u64,
    pub app_role: String,
    /// One of [`custom_app_outcome`].
    pub outcome: String,
    pub error_kind: String,
    pub error_detail: String,
    /// W3C ids of the platform-trace span this row belongs to — `''` when the
    /// process has no OpenTelemetry layer. What lets HyperDX click from this
    /// row to the request's trace.
    pub trace_id: String,
    pub span_id: String,
}

/// One durable `ctx.log()` / `console.*` line from an Oxy Function.
#[derive(Debug, Clone)]
pub struct CustomAppLogRecord {
    pub timestamp_ms: i64,
    pub org_id: String,
    pub app_id: String,
    pub build_id: String,
    pub invocation_id: String,
    pub request_id: String,
    pub function_name: String,
    pub mode: String,
    pub log_level: String,
    /// Position within the invocation, so lines that share a millisecond still
    /// read back in the order the function wrote them.
    pub seq: u32,
    pub message: String,
    pub trace_id: String,
    pub span_id: String,
}

/// Success/failure counts for one app over one window — the raw material for
/// an availability SLI and its error budget.
#[derive(Debug, Clone, PartialEq)]
pub struct AppAvailabilityWindow {
    /// Length of the window this covers.
    pub window_minutes: u32,
    pub total: u64,
    /// Requests that did not succeed, by the `outcome` taxonomy — explicit,
    /// implicit and policy failures all count against availability.
    pub failed: u64,
}

impl AppAvailabilityWindow {
    /// Fraction of requests that failed. An empty window has **no opinion**
    /// (`None`) rather than 0.0 — "no traffic" and "no failures" are different
    /// facts, and collapsing them makes a dead app look perfectly healthy.
    pub fn failure_ratio(&self) -> Option<f64> {
        if self.total == 0 {
            return None;
        }
        Some(self.failed as f64 / self.total as f64)
    }
}

/// One uncaught browser error, with the text an engineer needs.
///
/// Distinct from a `CustomAppEventRecord` of kind `client`: that one records
/// *that* an error happened, for the availability signal. This one records
/// *what* it was, and lives under a shorter retention and a tighter gate.
#[derive(Debug, Clone)]
pub struct CustomAppClientErrorRecord {
    pub timestamp_ms: i64,
    pub org_id: String,
    pub app_id: String,
    pub build_id: String,
    pub session_id: String,
    pub user_id: String,
    pub error_name: String,
    pub message: String,
    pub stack: String,
    /// Grouping key over the normalised stack, computed client-side so the same
    /// fault recurring in a render loop arrives deduped rather than 400×.
    pub stack_hash: String,
    pub path: String,
    /// `error` (uncaught) or `unhandledrejection`.
    pub kind: String,
    pub user_agent: String,
    /// The trace the browser was in when it threw, when the failing call
    /// carried one (the SDK stamps `traceId` on a failed invoke). The browser
    /// has no span of its own, so `span_id` stays empty.
    pub trace_id: String,
    pub span_id: String,
}

/// One distinct client error, with its occurrence count over the window.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientErrorGroup {
    pub stack_hash: String,
    pub error_name: String,
    pub message: String,
    /// Raw (still minified) stack from the most recent occurrence. Resolution
    /// against the build's source map happens at read time, in the app layer —
    /// this crate has no idea what a source map is.
    pub stack: String,
    pub build_id: String,
    pub path: String,
    pub kind: String,
    pub occurrences: u64,
    pub sessions: u64,
    pub first_seen: String,
    pub last_seen: String,
}

/// One persisted Oxy Function log line, as read back.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionLogRow {
    pub timestamp: String,
    pub build_id: String,
    pub invocation_id: String,
    pub request_id: String,
    pub function_name: String,
    pub mode: String,
    pub log_level: String,
    pub seq: u32,
    pub message: String,
    pub trace_id: String,
}
