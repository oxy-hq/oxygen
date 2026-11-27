//! Execution Analytics Module
//!
//! Provides types and storage for execution analytics data,
//! tracking verified vs generated executions across different tool types.
//!
//! Moved here from `oxy::execution_analytics` (depth stage C, D6, 2026-11-27):
//! it is the response shape of this crate's store, and it needs nothing from
//! `oxy`. `oxy::execution_analytics` re-exports it for one release.

mod storage;
mod types;

pub use storage::ExecutionAnalyticsStorage;
pub use types::{
    AgentExecutionStats, ExecutionCostResponse, ExecutionDetail, ExecutionListResponse,
    ExecutionSummary, ExecutionTimeBucket, ExecutionType, HistogramBucket,
    LatencyHistogramResponse, LatencyPercentilePoint, LatencyPercentilesResponse, LatencyTriple,
    ModelCost, SourceType, model_cost_usd,
};
