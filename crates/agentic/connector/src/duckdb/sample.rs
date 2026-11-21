//! `execute_query`'s sampler: one returning statement materialized into a
//! temp table once, then read back as a bounded sample, the full row count
//! and per-column stats. Shared by the plain path and the read-only one
//! (`super::locked`), so both shape an `ExecutionResult` the same way.

use agentic_core::result::{QueryResult, QueryRow};
use duckdb::{Connection, types::Value};

use super::conversion::duckdb_to_cell;
use super::schema::describe_table;
use crate::connector::{ColumnStats, ConnectorError, ExecutionResult, ResultSummary};

/// Sample `sql`, one statement that returns rows, on `conn`.
pub(super) fn sample(
    conn: &Connection,
    sql: &str,
    sample_limit: u64,
) -> Result<ExecutionResult, ConnectorError> {
    let tmp = "_agentic_tmp";

    // 1. Create the temp table once from the final (returning) query.
    conn.execute_batch(&format!("DROP TABLE IF EXISTS {tmp};"))
        .map_err(|e| ConnectorError::query_failed(sql.to_string(), e.to_string()))?;

    conn.execute_batch(&format!("CREATE OR REPLACE TEMP TABLE {tmp} AS ({sql});"))
        .map_err(|e| ConnectorError::query_failed(sql.to_string(), e.to_string()))?;

    // 2. Total row count.
    let total_row_count: u64 = {
        let count_sql = format!("SELECT COUNT(*) FROM {tmp}");
        conn.query_row(&count_sql, [], |row| row.get::<_, i64>(0))
            .map_err(|e| ConnectorError::query_failed(count_sql, e.to_string()))? as u64
    };

    // 3a. Column names — use DESCRIBE on the temp table because duckdb-rs
    //     requires the statement to be executed before column_count()
    //     and column_names() are available, and we need them first.
    let described = describe_table(&conn, tmp)
        .map_err(|e| ConnectorError::query_failed(format!("DESCRIBE {tmp}"), e.to_string()))?;
    let column_names: Vec<String> = described.iter().map(|(name, _)| name.clone()).collect();
    let column_types: Vec<String> = described.iter().map(|(_, ty)| ty.clone()).collect();

    // 3b. Sample rows.
    let col_count = column_names.len();
    let sample_rows: Vec<QueryRow> = {
        let sample_sql = format!("SELECT * FROM {tmp} LIMIT {sample_limit}");
        let mut stmt = conn
            .prepare(&sample_sql)
            .map_err(|e| ConnectorError::query_failed(sample_sql.clone(), e.to_string()))?;

        stmt.query_map([], |row| {
            let cells = (0..col_count)
                .map(|i| row.get::<_, Value>(i).map(duckdb_to_cell))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(QueryRow(cells))
        })
        .map_err(|e| ConnectorError::query_failed(sample_sql.clone(), e.to_string()))?
        .collect::<Result<Vec<_>, duckdb::Error>>()
        .map_err(|e| ConnectorError::query_failed(sample_sql, e.to_string()))?
    };

    // 4. Per-column stats. Each column's query is best-effort: a
    // future DuckDB type, extension, or user-defined type without
    // MIN/MAX/COUNT(DISTINCT)/TRY_CAST AS DOUBLE support could
    // bind-error and otherwise tank the whole execute_query, surfacing
    // to the analytics agent as a generic "query failed" loop in the
    // reasoning trace. Match on the result and degrade just that
    // column's stats to None — same shape as the BigQuery connector.
    //
    // NB: the DuckDB version pinned today aggregates all built-in
    // complex types (MAP, LIST, STRUCT, UNION, BLOB, BIT) gracefully,
    // so this branch is defense-in-depth rather than a fix for an
    // observed bug.
    let mut col_stats: Vec<ColumnStats> = Vec::with_capacity(column_names.len());
    for (idx, col) in column_names.iter().enumerate() {
        let quoted = format!("\"{}\"", col.replace('"', "\"\""));
        let data_type = column_types.get(idx).cloned();
        let stat_sql = format!(
            "SELECT \
                COUNT(*) - COUNT({quoted}), \
                COUNT(DISTINCT {quoted}), \
                MIN({quoted}), \
                MAX({quoted}), \
                AVG(TRY_CAST({quoted} AS DOUBLE)), \
                STDDEV_POP(TRY_CAST({quoted} AS DOUBLE)) \
             FROM {tmp}"
        );

        match conn.query_row(&stat_sql, [], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Value>(2)?,
                row.get::<_, Value>(3)?,
                row.get::<_, Option<f64>>(4)?,
                row.get::<_, Option<f64>>(5)?,
            ))
        }) {
            Ok((null_count, distinct_count, min_v, max_v, mean, std_dev)) => {
                col_stats.push(ColumnStats {
                    name: col.clone(),
                    data_type,
                    null_count: null_count as u64,
                    distinct_count: Some(distinct_count as u64),
                    min: Some(duckdb_to_cell(min_v)),
                    max: Some(duckdb_to_cell(max_v)),
                    mean,
                    std_dev,
                });
            }
            Err(e) => {
                // TODO(test): the bundled DuckDB version aggregates every
                // built-in complex type successfully, so this arm has no
                // realistic reproducer in `tests/integration/duckdb_tests.rs`. If a
                // future change here breaks the degraded-stats shape it
                // won't be caught — revisit once an extension or UDT
                // exists that genuinely bind-errors.
                tracing::debug!(
                    column = %col,
                    column_type = ?data_type,
                    error = %e,
                    "DuckDB per-column stats query failed; degrading to empty stats \
                     (null_count is unknown, reported as 0)"
                );
                col_stats.push(ColumnStats {
                    name: col.clone(),
                    data_type,
                    null_count: 0,
                    distinct_count: None,
                    min: None,
                    max: None,
                    mean: None,
                    std_dev: None,
                });
            }
        }
    }

    // 5. Clean up.
    let _ = conn.execute_batch(&format!("DROP TABLE IF EXISTS {tmp};"));

    let truncated = (sample_rows.len() as u64) < total_row_count;
    Ok(ExecutionResult {
        result: QueryResult {
            columns: column_names,
            rows: sample_rows,
            total_row_count,
            truncated,
        },
        summary: ResultSummary {
            row_count: total_row_count,
            columns: col_stats,
        },
    })
}
