//! The warehouse half of a rollup build: run a build plan's statements and pull
//! a built rollup back, through the workspace's connector.
//!
//! Up from `agentic_semantic::preagg` (2026-11-26), whose only callers were
//! here ([`crate::rebuild`]), so the semantic helpers need not depend on the
//! connector crate (`internal-docs/crate-tiers.md`). The plan itself and the
//! local Parquet write stay there.

use std::path::Path;
use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use agentic_semantic::error::SemanticError;

/// Execute all statements in a `BuildPlan` against a warehouse connector.
pub async fn execute_build_plan(
    connector: &Arc<dyn DatabaseConnector>,
    plan: &oxy_airlayer_compat::preagg::BuildPlan,
) -> Result<(), SemanticError> {
    let run = async |stmt: &String| -> Result<(), SemanticError> {
        connector.execute_statement(stmt).await.map_err(|e| {
            SemanticError::Runtime(format!("build plan statement failed: {e}\nSQL: {stmt}"))
        })?;
        Ok(())
    };

    // Three phases, and the order is load-bearing. `prelude_len` statements
    // create the schema and `__manifest`; `migrations` ALTER that table; the
    // rest write to it. Run in any other order the migrations are DDL against
    // a table that does not exist yet, and the writes hit a manifest missing
    // the columns they set.
    let prelude_len = plan.prelude_len.min(plan.statements.len());
    for stmt in &plan.statements[..prelude_len] {
        run(stmt).await?;
    }
    // Best-effort by contract: on a manifest that already has the columns
    // these are EXPECTED to fail, and not every dialect can express
    // `ADD COLUMN IF NOT EXISTS`. A real problem surfaces on the write below.
    for stmt in &plan.migrations {
        if let Err(e) = run(stmt).await {
            tracing::debug!(error = %e, "manifest migration declined (expected when already applied)");
        }
    }
    for stmt in &plan.statements[prelude_len..] {
        run(stmt).await?;
    }
    Ok(())
}

/// Download a single pre-aggregation rollup from the warehouse into a Parquet file.
///
/// Returns `true` if the file was written, `false` if the rollup had no rows
/// (caller should skip `agentic_semantic::preagg::hot_swap_parquet` in that case).
/// Writes to `temp_path`. Call `hot_swap_parquet` after to atomically rename it
/// to the final path.
/// Maximum number of rows `pull_rollup` will materialise into the local
/// Parquet cache in a single pass. The current implementation buffers every
/// row into memory and serialises a single `VALUES (...)` SQL statement into
/// DuckDB, so anything beyond this would build a multi-GB SQL string and
/// likely OOM the worker. When this trips, the rollup is too large for the
/// current pipeline — surface a clear error so operators size their
/// pre-aggregations down or wait for the streaming Arrow/Appender rewrite.
pub const PULL_ROLLUP_ROW_LIMIT: u64 = 500_000;

pub async fn pull_rollup(
    connector: &Arc<dyn DatabaseConnector>,
    table_name: &str,
    temp_path: &Path,
) -> Result<bool, SemanticError> {
    let sql = format!("SELECT * FROM {table_name}");
    let result = connector
        .execute_query(&sql, PULL_ROLLUP_ROW_LIMIT)
        .await
        .map_err(|e| SemanticError::Runtime(format!("pull failed for {table_name}: {e}")))?;

    if result.result.truncated {
        return Err(SemanticError::Runtime(format!(
            "pull_rollup: rollup {table_name} exceeds PULL_ROLLUP_ROW_LIMIT ({PULL_ROLLUP_ROW_LIMIT} rows). \
             The current materializer buffers all rows into a single VALUES SQL statement; \
             reduce the rollup's grain or wait for the streaming Arrow rewrite."
        )));
    }

    agentic_semantic::preagg::write_result_to_parquet(&result.result, temp_path)
}
