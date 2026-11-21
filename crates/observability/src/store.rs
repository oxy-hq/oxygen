//! Trait definition for observability storage backends.
//!
//! The [`ObservabilityStore`] trait abstracts the storage backend for
//! observability data (traces, metrics, execution analytics).
//!
//! It has exactly one production implementation — `ClickHouseObservabilityStorage`
//! — and the multi-engine era it was built for is over (see
//! `internal-docs/observability-analytics.md`). What the trait still buys is a
//! seam consumers can be tested against without a database: the batching bridge
//! in `telemetry.rs` exercises a `RecordingStore` mock through it.

use std::collections::HashMap;

use async_trait::async_trait;
use oxy_shared::errors::OxyError;

use crate::scope::WorkspaceScope;
use crate::types::{
    AgentExecutionStatsData, AppAvailabilityWindow, ClientErrorGroup, ClusterInfoRow,
    ClusterMapDataRow, CustomAppClientErrorRecord, CustomAppEventRecord, CustomAppLogRecord,
    ExecutionListData, ExecutionSummaryData, ExecutionTimeBucketData, FunctionLogRow,
    LatencyHistogramData, LatencyPercentilesData, MetricAnalyticsData, MetricDetailData,
    MetricUsageRecord, MetricsListData, ModelUsageData, OrgAppTotals, OrgAppTraffic, SpanRecord,
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

    /// List `scope`'s traces with pagination and filtering.
    /// Returns `(traces, total_count)`.
    ///
    /// Every read in this section is confined to one workspace: the agent
    /// tables are shared by all tenants, and a read that does not say whose
    /// rows it wants returns everyone's. See [`crate::scope`].
    async fn list_traces(
        &self,
        scope: &WorkspaceScope,
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
        scope: &WorkspaceScope,
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
        self.list_traces(scope, limit, offset, agent_ref, status, duration_filter)
            .await
    }

    /// Every span of `trace_id`, if the trace is `scope`'s. Another
    /// workspace's trace id answers the same as one that does not exist.
    async fn get_trace_detail(
        &self,
        scope: &WorkspaceScope,
        trace_id: &str,
    ) -> Result<Vec<TraceDetailRow>, OxyError>;

    /// Get embeddings with classification data for cluster map visualization,
    /// for questions asked in `scope`.
    async fn get_cluster_map_data(
        &self,
        scope: &WorkspaceScope,
        days: u32,
        limit: usize,
        source: Option<&str>,
    ) -> Result<Vec<ClusterMapDataRow>, OxyError>;

    async fn get_cluster_infos(&self) -> Result<Vec<ClusterInfoRow>, OxyError>;

    /// Get trace enrichment data (status, duration) for those of `trace_ids`
    /// that are `scope`'s.
    async fn get_trace_enrichments(
        &self,
        scope: &WorkspaceScope,
        trace_ids: &[String],
    ) -> Result<Vec<TraceEnrichmentRow>, OxyError>;

    // ── Metrics ───────────────────────────────────────────────────────────

    async fn store_metric_usages(&self, metrics: Vec<MetricUsageRecord>) -> Result<(), OxyError>;

    /// Get analytics summary for the last N days.
    async fn get_metrics_analytics(
        &self,
        scope: &WorkspaceScope,
        days: u32,
    ) -> Result<MetricAnalyticsData, OxyError>;

    async fn get_metrics_list(
        &self,
        scope: &WorkspaceScope,
        days: u32,
        limit: usize,
        offset: usize,
    ) -> Result<MetricsListData, OxyError>;

    async fn get_metric_detail(
        &self,
        scope: &WorkspaceScope,
        metric_name: &str,
        days: u32,
    ) -> Result<MetricDetailData, OxyError>;

    // ── Execution Analytics ───────────────────────────────────────────────

    async fn get_execution_summary(
        &self,
        scope: &WorkspaceScope,
        days: u32,
    ) -> Result<ExecutionSummaryData, OxyError>;

    /// Get execution time series (daily buckets).
    async fn get_execution_time_series(
        &self,
        scope: &WorkspaceScope,
        days: u32,
    ) -> Result<Vec<ExecutionTimeBucketData>, OxyError>;

    async fn get_execution_agent_stats(
        &self,
        scope: &WorkspaceScope,
        days: u32,
        limit: usize,
    ) -> Result<Vec<AgentExecutionStatsData>, OxyError>;

    /// Get paginated execution details.
    async fn get_execution_list(
        &self,
        scope: &WorkspaceScope,
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
        _scope: &WorkspaceScope,
        _days: u32,
    ) -> Result<LatencyPercentilesData, OxyError> {
        Ok(LatencyPercentilesData::default())
    }

    /// Latency histogram (log-spaced buckets) plus p50/p95/p99 markers.
    async fn get_latency_histogram(
        &self,
        _scope: &WorkspaceScope,
        _days: u32,
    ) -> Result<LatencyHistogramData, OxyError> {
        Ok(LatencyHistogramData::default())
    }

    /// Per-model LLM token usage (for cost estimation). Aggregates `llm` spans.
    async fn get_model_usage(
        &self,
        _scope: &WorkspaceScope,
        _days: u32,
    ) -> Result<Vec<ModelUsageData>, OxyError> {
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
    /// `environment` empty means production only, which is what every read
    /// was before the argument; a name means that app environment's lines and
    /// no other's.
    #[allow(clippy::too_many_arguments)]
    async fn get_function_logs(
        &self,
        _org_id: &str,
        _app_id: &str,
        _hours: u32,
        _limit: u32,
        _invocation_id: &str,
        _request_id: &str,
        _environment: &str,
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

    /// Per app (keyed by `app_id`), how many sessions active in the last
    /// `window_minutes` were last seen on each build — from the platform
    /// runtime's `oxy-app-ready` / `oxy-update` rows. An app absent from the map
    /// had no session report a build; `Err` (or capture off) is "not measured",
    /// and a caller must keep the two apart.
    async fn get_fleet_build_sessions(
        &self,
        _apps: &[(String, String)],
        _window_minutes: u32,
    ) -> Result<HashMap<String, Vec<crate::types::AppBuildSessions>>, OxyError> {
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

    /// One org's custom-app performance over `days`: totals with the latency
    /// tail, the failure classes apart, a daily series, the per-app breakdown
    /// and the slowest routes. What Admin → Observability's Performance panel
    /// reads.
    ///
    /// An org that served nothing is `Ok` with zeros; a deployment with no
    /// observability store configured never reaches here, and the surface
    /// names the meters absent rather than drawing those zeros. The default
    /// impl below is what a mock store answers, so a consumer can be tested
    /// without a ClickHouse.
    async fn get_org_app_traffic(
        &self,
        _org_id: &str,
        _days: u32,
    ) -> Result<OrgAppTraffic, OxyError> {
        Ok(OrgAppTraffic::default())
    }

    /// The same headline for many orgs at once, keyed by `org_id` — the fleet
    /// list's performance columns.
    ///
    /// Batched for the reason `get_fleet_availability` is: one statement over
    /// a whole page of orgs, rather than a round trip each. Only the fields
    /// the list renders are filled (requests, failures, sessions, people,
    /// apps, bytes and p95); the rest stay at their zero default, and a caller
    /// that needs them asks [`Self::get_org_app_traffic`] for one org.
    ///
    /// An org absent from the map served nothing in the window.
    ///
    /// `offset_days` shifts the window back: `0` is the last `days` days, and
    /// `days` is the `days` before those — the prior period the fleet list
    /// compares against.
    async fn get_fleet_org_traffic(
        &self,
        _org_ids: &[String],
        _days: u32,
        _offset_days: u32,
    ) -> Result<HashMap<String, OrgAppTotals>, OxyError> {
        Ok(HashMap::new())
    }

    /// Function wall time across the **whole fleet**, in milliseconds — the
    /// denominator for apportioning the serving bill.
    ///
    /// Unfenced on purpose: the bill is over every tenant, so a share computed
    /// against a bounded subset would overstate it by whatever the caller
    /// cannot see. One scalar, carrying no tenant identity. `offset_days`
    /// shifts the window back, as on [`Self::get_fleet_org_traffic`].
    async fn get_fleet_wall_ms(&self, _days: u32, _offset_days: u32) -> Result<u64, OxyError> {
        Ok(0)
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────

    /// Gracefully shut down the storage backend, flushing any buffered data.
    async fn shutdown(&self);
}
