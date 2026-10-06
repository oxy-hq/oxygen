//! Metric usage storage
//!
//! Delegates to the ObservabilityStore backend for metric usage data.

use oxy_observability::WorkspaceScope;
use std::sync::Arc;

use oxy_observability::ObservabilityStore;
use oxy_shared::errors::OxyError;

use super::types::{
    ContextTypeBreakdown, MetricAnalytics, MetricAnalyticsResponse, MetricDetailResponse,
    MetricUsage, MetricsListResponse, RecentUsage, RelatedMetric, SourceTypeBreakdown,
    UsageTrendPoint,
};

/// Storage client for metric usage data
pub struct MetricStorage {
    storage: Arc<dyn ObservabilityStore>,
}

impl std::fmt::Debug for MetricStorage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricStorage")
            .field("storage", &self.storage)
            .finish()
    }
}

impl MetricStorage {
    /// Create a new storage client wrapping an ObservabilityStore instance.
    pub fn new(storage: Arc<dyn ObservabilityStore>) -> Self {
        Self { storage }
    }

    /// Create from the global observability storage singleton.
    /// Returns an error naming the real gate (`OXY_OBSERVABILITY_BACKEND`) when
    /// the global has not been initialized — the fourth call site that had its
    /// own, vaguer copy of this message.
    pub fn from_global() -> Result<Self, OxyError> {
        let storage = oxy_observability::global::require_global()?.clone();
        Ok(Self { storage })
    }

    pub async fn store_metrics(&self, metrics: &[MetricUsage]) -> Result<(), OxyError> {
        if metrics.is_empty() {
            return Ok(());
        }

        let records: Vec<oxy_observability::MetricUsageRecord> = metrics
            .iter()
            .map(|m| {
                let context_types_json = serde_json::to_string(
                    &m.context_types
                        .iter()
                        .map(|ct| ct.as_str())
                        .collect::<Vec<_>>(),
                )
                .unwrap_or_else(|_| "[]".to_string());

                oxy_observability::MetricUsageRecord {
                    metric_name: m.metric_name.clone(),
                    source_type: m.source_type.as_str().to_string(),
                    source_ref: m.source_ref.clone(),
                    context_types: context_types_json,
                    trace_id: m.trace_id.clone(),
                    context: m.context.clone().unwrap_or_default(),
                }
            })
            .collect();

        self.storage.store_metric_usages(records).await
    }

    pub async fn store_metric(&self, metric: &MetricUsage) -> Result<(), OxyError> {
        self.store_metrics(std::slice::from_ref(metric)).await
    }

    /// Get analytics summary for a time period
    pub async fn get_analytics(
        &self,
        scope: &WorkspaceScope,
        days: u32,
    ) -> Result<MetricAnalyticsResponse, OxyError> {
        let data = self.storage.get_metrics_analytics(scope, days).await?;

        Ok(MetricAnalyticsResponse {
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
        })
    }

    pub async fn get_metrics_list(
        &self,
        scope: &WorkspaceScope,
        days: u32,
        limit: usize,
        offset: usize,
    ) -> Result<MetricsListResponse, OxyError> {
        let data = self
            .storage
            .get_metrics_list(scope, days, limit, offset)
            .await?;

        let metrics: Vec<MetricAnalytics> = data
            .metrics
            .into_iter()
            .map(|r| MetricAnalytics {
                name: r.name,
                count: r.count,
                last_used: Some(r.last_used),
                trend: None,
            })
            .collect();

        Ok(MetricsListResponse {
            metrics,
            total: data.total,
            limit: data.limit,
            offset: data.offset,
        })
    }

    pub async fn get_metric_detail(
        &self,
        scope: &WorkspaceScope,
        metric_name: &str,
        days: u32,
    ) -> Result<MetricDetailResponse, OxyError> {
        let data = self
            .storage
            .get_metric_detail(scope, metric_name, days)
            .await?;

        let usage_trend: Vec<UsageTrendPoint> = data
            .usage_trend
            .into_iter()
            .map(|r| UsageTrendPoint {
                date: r.date,
                count: r.count,
            })
            .collect();

        let related_metrics: Vec<RelatedMetric> = data
            .related_metrics
            .into_iter()
            .map(|r| RelatedMetric {
                name: r.name,
                co_occurrence_count: r.co_occurrence_count,
            })
            .collect();

        let recent_usage: Vec<RecentUsage> = data
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

        Ok(MetricDetailResponse {
            name: data.name,
            total_queries: data.total_queries,
            trend_vs_last_period: data.trend_vs_last_period,
            via_agent: data.via_agent,
            via_workflow: data.via_workflow,
            usage_trend,
            related_metrics,
            recent_usage,
        })
    }
}
