//! Host adapter for [`agentic_pipeline::AnalyticsMetricSink`] — writes
//! Tier 1 metric usage records (the measures and dimensions from a
//! resolved semantic query) into Oxy's
//! [`oxy_observability::ObservabilityStore`].
//!
//! This is the ONLY place where the agentic side meets
//! `oxy_observability`. The domain crate (`agentic-analytics`) depends
//! only on the port trait, not on the infrastructure.

use std::collections::HashSet;
use std::sync::Arc;

use agentic_pipeline::AnalyticsMetricSink;
use oxy_observability::{MetricUsageRecord, ObservabilityStore, current_trace_id};

/// Source-type label written to `observability_metric_usage.source_type`.
/// Kept in sync with [`oxy::metrics::SourceType::Analytics`].
const SOURCE_TYPE: &str = "analytics";

/// Adapter: implements [`AnalyticsMetricSink`] by writing into the
/// globally-registered [`ObservabilityStore`]. `Option` because the
/// store might not be registered (enterprise-less runs, tests).
#[derive(Debug, Default)]
pub struct OxyAnalyticsMetricSink;

impl OxyAnalyticsMetricSink {
    pub fn new() -> Self {
        Self
    }
}

impl AnalyticsMetricSink for OxyAnalyticsMetricSink {
    fn record_analytics_query(
        &self,
        agent_id: &str,
        question: &str,
        measures: &[String],
        dimensions: &[String],
        sql: &str,
    ) {
        if measures.is_empty() && dimensions.is_empty() {
            tracing::debug!("OxyAnalyticsMetricSink: no measures or dimensions — skipping");
            return;
        }

        let Some(store) = oxy_observability::global::get_global() else {
            tracing::warn!(
                "OxyAnalyticsMetricSink: skipping — global ObservabilityStore not initialized"
            );
            return;
        };
        let store: Arc<dyn ObservabilityStore> = Arc::clone(store);

        let trace_id = current_trace_id().unwrap_or_default();
        if trace_id.is_empty() {
            tracing::warn!(
                "OxyAnalyticsMetricSink: skipping — current_trace_id() returned empty \
                 (no active SpanCollectorLayer span in scope)"
            );
            return;
        }

        let context_json = build_context_json(question, sql, measures, dimensions);
        let context_types = r#"["Question","SQL","SemanticQuery"]"#.to_string();
        let source_ref = agent_id.to_string();

        let mut records: Vec<MetricUsageRecord> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for name in measures.iter().chain(dimensions.iter()) {
            if !seen.insert(name.clone()) {
                continue;
            }
            records.push(MetricUsageRecord {
                metric_name: name.clone(),
                source_type: SOURCE_TYPE.to_string(),
                source_ref: source_ref.clone(),
                context: context_json.clone(),
                context_types: context_types.clone(),
                trace_id: trace_id.clone(),
            });
        }

        if records.is_empty() {
            return;
        }

        // Fire-and-forget: the pipeline must not block on this write,
        // and a store failure must never surface as a pipeline error.
        tokio::spawn(async move {
            if let Err(e) = store.store_metric_usages(records).await {
                tracing::warn!(error = %e, "failed to write analytics metric usage records");
            }
        });
    }
}

fn build_context_json(
    question: &str,
    sql: &str,
    measures: &[String],
    dimensions: &[String],
) -> String {
    let semantic = serde_json::json!([{
        "measures": measures,
        "dimensions": dimensions,
    }]);
    let items = serde_json::json!([
        { "type": "question", "content": question },
        { "type": "sql", "content": sql },
        { "type": "semantic", "content": semantic },
    ]);
    serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string())
}
