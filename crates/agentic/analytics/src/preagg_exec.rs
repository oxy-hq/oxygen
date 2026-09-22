//! Execute a `SolutionPayload::Preaggregation` via in-memory DuckDB.
//!
//! Goes straight from DuckDB rows to `ExecutionResult` (no
//! `serde_json::Value` intermediate), and pushes a `LIMIT` into the SQL so
//! DuckDB stops scanning once the sample is filled. When the sample comes back
//! SHORT of that limit it is the whole population, so `total_row_count` is just
//! its length; only a sample that fills the limit pays for a separate
//! `COUNT(*)`, which is what lets the interpreting prompt and the UI report the
//! true cardinality and badge the result as truncated.

use agentic_connector::{ColumnStats, ExecutionResult, ResultSummary};
use agentic_core::hub_task::spawn_blocking_with_hub;
use agentic_core::result::QueryResult;

/// Execute `preagg_sql` against `source` via in-memory DuckDB, returning up to
/// `sample_limit` rows plus the true total row count.
///
/// Runs the blocking DuckDB call on a tokio blocking thread — which matters
/// more for a blob source than a local one, since that read goes over the
/// network.
pub(crate) async fn execute_rollup(
    preagg_sql: String,
    source: agentic_semantic::compile::PreaggSource,
    sample_limit: u64,
) -> Result<ExecutionResult, String> {
    let (columns, rows, total_row_count) = spawn_blocking_with_hub(move || {
        agentic_semantic::preagg::execute_preagg_sql_typed(&preagg_sql, &source, sample_limit)
    })
    .await
    .map_err(|e| format!("preagg task panicked: {e}"))?
    .map_err(|e| e.to_string())?;

    let truncated = (rows.len() as u64) < total_row_count;

    let summary = ResultSummary {
        row_count: total_row_count,
        // DuckDB on-the-fly stats are out of scope for the cache read path;
        // emit one entry per column with type-less defaults so downstream
        // consumers that expect `summary.columns.len() == columns.len()`
        // (Execution Analytics tab, validator) don't panic.
        columns: columns
            .iter()
            .map(|name| ColumnStats {
                name: name.clone(),
                data_type: None,
                null_count: 0,
                distinct_count: None,
                min: None,
                max: None,
                mean: None,
                std_dev: None,
            })
            .collect(),
    };

    Ok(ExecutionResult {
        result: QueryResult {
            columns,
            rows,
            total_row_count,
            truncated,
        },
        summary,
    })
}
