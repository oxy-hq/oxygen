//! Execution analytics storage
//!
//! Delegates to the observability store for execution analytics data.

use oxy_observability::WorkspaceScope;
use std::sync::Arc;

use oxy_observability::{
    AgentExecutionStatsData, ExecutionDetailData, ExecutionListData, ExecutionSummaryData,
    ExecutionTimeBucketData,
};
use oxy_shared::errors::OxyError;

use super::types::{
    AgentExecutionStats, ExecutionDetail, ExecutionListResponse, ExecutionSummary,
    ExecutionTimeBucket,
};

/// Storage implementation for execution analytics.
///
/// Wraps an [`ObservabilityStore`] reference and converts its result types into the
/// API-facing types defined in [`super::types`].
pub struct ExecutionAnalyticsStorage {
    storage: Arc<dyn oxy_observability::ObservabilityStore>,
}

impl ExecutionAnalyticsStorage {
    pub fn new(storage: Arc<dyn oxy_observability::ObservabilityStore>) -> Self {
        Self { storage }
    }

    /// Create from the global observability storage singleton.
    /// Returns an error if the global has not been initialized.
    pub fn from_global() -> Result<Self, OxyError> {
        let storage = oxy_observability::global::require_global()?.clone();
        Ok(Self { storage })
    }

    pub async fn get_summary(
        &self,
        scope: &WorkspaceScope,
        days: u32,
    ) -> Result<ExecutionSummary, OxyError> {
        let data: ExecutionSummaryData = self.storage.get_execution_summary(scope, days).await?;

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

        Ok(ExecutionSummary {
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
        })
    }

    pub async fn get_time_series(
        &self,
        scope: &WorkspaceScope,
        days: u32,
    ) -> Result<Vec<ExecutionTimeBucket>, OxyError> {
        let rows: Vec<ExecutionTimeBucketData> =
            self.storage.get_execution_time_series(scope, days).await?;

        Ok(rows
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
            .collect())
    }

    /// Get per-agent execution statistics
    pub async fn get_agent_stats(
        &self,
        scope: &WorkspaceScope,
        days: u32,
        limit: usize,
    ) -> Result<Vec<AgentExecutionStats>, OxyError> {
        let rows: Vec<AgentExecutionStatsData> = self
            .storage
            .get_execution_agent_stats(scope, days, limit)
            .await?;

        Ok(rows
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
            .collect())
    }

    /// Get paginated execution details
    pub async fn get_executions(
        &self,
        scope: &WorkspaceScope,
        days: u32,
        limit: usize,
        offset: usize,
        execution_type: Option<&str>,
        is_verified: Option<bool>,
        source_ref: Option<&str>,
        status: Option<&str>,
    ) -> Result<ExecutionListResponse, OxyError> {
        let data: ExecutionListData = self
            .storage
            .get_execution_list(
                scope,
                days,
                limit,
                offset,
                execution_type,
                is_verified,
                source_ref,
                status,
            )
            .await?;

        let executions = data
            .executions
            .into_iter()
            .map(|row: ExecutionDetailData| {
                let error = non_empty(row.error);

                ExecutionDetail {
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
                    error,
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
                }
            })
            .collect();

        Ok(ExecutionListResponse {
            executions,
            total: data.total,
            limit: data.limit,
            offset: data.offset,
        })
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() { None } else { Some(s) }
}
