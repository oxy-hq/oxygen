//! Trait definition for observability storage backends.
//!
//! The [`ObservabilityStore`] trait abstracts the storage backend for
//! observability data (traces, intents, metrics, execution analytics).
//!
//! It has exactly one production implementation — `ClickHouseObservabilityStorage`
//! — and the multi-engine era it was built for is over (see
//! `internal-docs/observability-analytics.md`). What the trait still buys is a
//! seam consumers can be tested against without a database: the batching bridge
//! in `telemetry.rs` exercises a `RecordingStore` mock through it.

use std::collections::HashMap;

use async_trait::async_trait;
use oxy_shared::errors::OxyError;

use crate::intent_types::IntentCluster;
use crate::types::{
    AgentExecutionStatsData, AppAvailabilityWindow, ClientErrorGroup, ClusterInfoRow,
    ClusterMapDataRow, CustomAppClientErrorRecord, CustomAppEventRecord, CustomAppLogRecord,
    ExecutionListData, ExecutionSummaryData, ExecutionTimeBucketData, FunctionLogRow,
    IntentAnalyticsRow, LatencyHistogramData, LatencyPercentilesData, MetricAnalyticsData,
    MetricDetailData, MetricUsageRecord, MetricsListData, ModelUsageData, SpanRecord,
    TraceDetailRow, TraceEnrichmentRow, TraceRow,
};

/// Abstraction over an observability storage backend.
///
/// All methods are async and return `Result<T, OxyError>`. Implementors must
/// be `Send + Sync + Debug` so the trait object can be shared across threads
/// and stored in application state.
#[async_trait]
pub trait ObservabilityStore: Send + Sync + std::fmt::Debug {
    // ── Traces ────────────────────────────────────────────────────────────

    /// List traces with pagination and filtering.
    /// Returns `(traces, total_count)`.
    async fn list_traces(
        &self,
        limit: i64,
        offset: i64,
        agent_ref: Option<&str>,
        status: Option<&str>,
        duration_filter: Option<&str>,
    ) -> Result<(Vec<TraceRow>, i64), OxyError>;

    /// List traces with the above filters plus a free-text `search` and an
    /// absolute `from_ts`/`to_ts` epoch-second range (Theme 3). Defaults to
    /// [`Self::list_traces`] (ignoring the extra filters), so only the ClickHouse
    /// backend implements the full query.
    #[allow(clippy::too_many_arguments)]
    async fn search_traces(
        &self,
        limit: i64,
        offset: i64,
        agent_ref: Option<&str>,
        status: Option<&str>,
        duration_filter: Option<&str>,
        search: Option<&str>,
        from_ts: Option<i64>,
        to_ts: Option<i64>,
    ) -> Result<(Vec<TraceRow>, i64), OxyError> {
        let _ = (search, from_ts, to_ts);
        self.list_traces(limit, offset, agent_ref, status, duration_filter)
            .await
    }

    async fn get_trace_detail(&self, trace_id: &str) -> Result<Vec<TraceDetailRow>, OxyError>;

    /// Get embeddings with classification data for cluster map visualization.
    async fn get_cluster_map_data(
        &self,
        days: u32,
        limit: usize,
        source: Option<&str>,
    ) -> Result<Vec<ClusterMapDataRow>, OxyError>;

    async fn get_cluster_infos(&self) -> Result<Vec<ClusterInfoRow>, OxyError>;

    /// Get trace enrichment data (status, duration) for a set of trace IDs.
    async fn get_trace_enrichments(
        &self,
        trace_ids: &[String],
    ) -> Result<Vec<TraceEnrichmentRow>, OxyError>;

    // ── Intents ───────────────────────────────────────────────────────────

    /// Fetch unprocessed questions from spans that lack classifications.
    /// Returns tuples of `(trace_id, question, source)`.
    async fn fetch_unprocessed_questions(
        &self,
        limit: usize,
    ) -> Result<Vec<(String, String, String)>, OxyError>;

    /// Load all embeddings from intent_classifications.
    /// Returns tuples of `(trace_id, question, embedding, intent_name, source)`.
    async fn load_embeddings(
        &self,
    ) -> Result<Vec<(String, String, Vec<f32>, String, String)>, OxyError>;

    /// Store clusters (replace all existing, then insert new ones).
    async fn store_clusters(&self, clusters: &[IntentCluster]) -> Result<(), OxyError>;

    async fn load_clusters(&self) -> Result<Vec<IntentCluster>, OxyError>;

    async fn store_classification(
        &self,
        trace_id: &str,
        question: &str,
        cluster_id: u32,
        intent_name: &str,
        confidence: f32,
        embedding: &[f32],
        source_type: &str,
        source: &str,
    ) -> Result<(), OxyError>;

    /// Upsert a classification keyed by `(trace_id, question)`. ClickHouse has
    /// no `UPDATE`, so the write is a plain insert that `ReplacingMergeTree`
    /// collapses on that key during background merges; until a merge runs both
    /// rows exist, which is why the read queries select `FINAL`.
    async fn update_classification(
        &self,
        trace_id: &str,
        question: &str,
        cluster_id: u32,
        intent_name: &str,
        confidence: f32,
        embedding: &[f32],
        source_type: &str,
        source: &str,
    ) -> Result<(), OxyError>;

    async fn get_intent_analytics(&self, days: u32) -> Result<Vec<IntentAnalyticsRow>, OxyError>;

    /// Get outlier questions (classified as "unknown").
    async fn get_outliers(&self, limit: usize) -> Result<Vec<(String, String)>, OxyError>;

    /// Load unknown classifications for incremental clustering.
    /// Returns tuples of `(trace_id, question, embedding, source)`.
    async fn load_unknown_classifications(
        &self,
    ) -> Result<Vec<(String, String, Vec<f32>, String)>, OxyError>;

    async fn get_unknown_count(&self) -> Result<usize, OxyError>;

    /// Update a single cluster (upsert).
    async fn update_cluster_record(&self, cluster: &IntentCluster) -> Result<(), OxyError>;

    async fn get_next_cluster_id(&self) -> Result<u32, OxyError>;

    // ── Metrics ───────────────────────────────────────────────────────────

    async fn store_metric_usages(&self, metrics: Vec<MetricUsageRecord>) -> Result<(), OxyError>;

    /// Get analytics summary for the last N days.
    async fn get_metrics_analytics(&self, days: u32) -> Result<MetricAnalyticsData, OxyError>;

    async fn get_metrics_list(
        &self,
        days: u32,
        limit: usize,
        offset: usize,
    ) -> Result<MetricsListData, OxyError>;

    async fn get_metric_detail(
        &self,
        metric_name: &str,
        days: u32,
    ) -> Result<MetricDetailData, OxyError>;

    // ── Execution Analytics ───────────────────────────────────────────────

    async fn get_execution_summary(&self, days: u32) -> Result<ExecutionSummaryData, OxyError>;

    /// Get execution time series (daily buckets).
    async fn get_execution_time_series(
        &self,
        days: u32,
    ) -> Result<Vec<ExecutionTimeBucketData>, OxyError>;

    async fn get_execution_agent_stats(
        &self,
        days: u32,
        limit: usize,
    ) -> Result<Vec<AgentExecutionStatsData>, OxyError>;

    /// Get paginated execution details.
    async fn get_execution_list(
        &self,
        days: u32,
        limit: usize,
        offset: usize,
        execution_type: Option<&str>,
        is_verified: Option<bool>,
        source_ref: Option<&str>,
        status: Option<&str>,
    ) -> Result<ExecutionListData, OxyError>;

    /// Latency percentiles (p50/p95/p99, ms) over executions — overall window
    /// plus a daily series. Defaults to empty for backends that don't implement
    /// it (only ClickHouse does).
    async fn get_latency_percentiles(
        &self,
        _days: u32,
    ) -> Result<LatencyPercentilesData, OxyError> {
        Ok(LatencyPercentilesData::default())
    }

    /// Latency histogram (log-spaced buckets) plus p50/p95/p99 markers.
    async fn get_latency_histogram(&self, _days: u32) -> Result<LatencyHistogramData, OxyError> {
        Ok(LatencyHistogramData::default())
    }

    /// Per-model LLM token usage (for cost estimation). Aggregates `llm` spans.
    async fn get_model_usage(&self, _days: u32) -> Result<Vec<ModelUsageData>, OxyError> {
        Ok(Vec::new())
    }

    // ── Span Ingestion ─────────────────────────────────────────────────────

    /// Insert span records directly (used by the tracing layer bridge).
    async fn insert_spans(&self, spans: Vec<SpanRecord>) -> Result<(), OxyError>;

    /// Insert a batch of custom-app wide events.
    ///
    /// Default is a no-op so a store that has no custom-app tables silently
    /// ignores them rather than failing the caller's request — this rides the
    /// serve hot path, and telemetry must never be able to break serving.
    async fn insert_custom_app_events(
        &self,
        _events: Vec<CustomAppEventRecord>,
    ) -> Result<(), OxyError> {
        Ok(())
    }

    /// Insert a batch of durable Oxy Function log lines.
    async fn insert_custom_app_logs(&self, _logs: Vec<CustomAppLogRecord>) -> Result<(), OxyError> {
        Ok(())
    }

    /// Insert a batch of client errors (message + stack). Separate from
    /// `insert_custom_app_events` because the two carry different retention and
    /// different exposure — see `CREATE_CUSTOM_APP_CLIENT_ERRORS_TABLE`.
    async fn insert_custom_app_client_errors(
        &self,
        _errors: Vec<CustomAppClientErrorRecord>,
    ) -> Result<(), OxyError> {
        Ok(())
    }

    /// Distinct client errors over a window, grouped by stack, newest first.
    /// `build_id` empty means "any build".
    async fn get_client_errors(
        &self,
        _org_id: &str,
        _app_id: &str,
        _hours: u32,
        _limit: u32,
        _build_id: &str,
    ) -> Result<Vec<ClientErrorGroup>, OxyError> {
        Ok(Vec::new())
    }

    /// Persisted Oxy Function log lines over a window, newest first.
    /// `invocation_id` / `request_id` empty mean "any"; both set means both.
    async fn get_function_logs(
        &self,
        _org_id: &str,
        _app_id: &str,
        _hours: u32,
        _limit: u32,
        _invocation_id: &str,
        _request_id: &str,
    ) -> Result<Vec<FunctionLogRow>, OxyError> {
        Ok(Vec::new())
    }

    /// Success/failure counts for one app across several windows, for the
    /// availability SLI. Returns one entry per requested window, in order.
    async fn get_app_availability(
        &self,
        _org_id: &str,
        _app_id: &str,
        _windows_minutes: &[u32],
    ) -> Result<Vec<AppAvailabilityWindow>, OxyError> {
        Ok(Vec::new())
    }

    /// The same counts for many apps at once, keyed by `app_id`, for the fleet
    /// health table. `apps` is `(org_id, app_id)` pairs.
    ///
    /// Batched rather than looped: the per-app call runs one query per window,
    /// so a page of the fleet table would otherwise cost `windows × apps` round
    /// trips.
    ///
    /// An app missing from the returned map served nothing. That is a zero, not
    /// missing data — the difference between "served nothing" and "we never
    /// asked" is carried by whether this returns `Ok` at all, and a caller that
    /// collapses the two reports an unmeasured app as an idle one.
    async fn get_fleet_availability(
        &self,
        _apps: &[(String, String)],
        _windows_minutes: &[u32],
    ) -> Result<HashMap<String, Vec<AppAvailabilityWindow>>, OxyError> {
        Ok(HashMap::new())
    }

    /// Request counts for the same window one cycle back, keyed by `app_id` —
    /// the baseline [`crate::heartbeat::evaluate`] compares against.
    async fn get_fleet_heartbeat_baseline(
        &self,
        _apps: &[(String, String)],
        _window_minutes: u32,
        _cycle_days: u32,
    ) -> Result<HashMap<String, u64>, OxyError> {
        Ok(HashMap::new())
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────

    /// Gracefully shut down the storage backend, flushing any buffered data.
    async fn shutdown(&self);
}
