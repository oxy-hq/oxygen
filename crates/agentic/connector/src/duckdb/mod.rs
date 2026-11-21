//! DuckDB connector implementation.
//!
//! Uses the temp-table pattern:
//! 1. `CREATE OR REPLACE TEMP TABLE _agentic_tmp AS ({sql})` — execute once.
//! 2. `SELECT COUNT(*) FROM _agentic_tmp` — total row count.
//! 3. `SELECT * FROM _agentic_tmp LIMIT {sample_limit}` — bounded sample.
//! 4. Per-column: `COUNT()-COUNT(col), COUNT(DISTINCT col), MIN, MAX,
//!    AVG(TRY_CAST(col AS DOUBLE)), STDDEV_POP(TRY_CAST(col AS DOUBLE))`.
//! 5. `DROP TABLE IF EXISTS _agentic_tmp` — cleanup.
//!
//! File loading registers Parquet/CSV files as temp views or materialized temp
//! tables via `from_directory()` / `from_files()`. `from_directory_cached()`
//! shares one database per unchanged directory across connectors instead
//! (see [`dir_cache`]).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use agentic_core::hub_task::spawn_blocking_with_hub;
use async_trait::async_trait;
use duckdb::{Connection, types::Value};
use slugify::slugify;

use agentic_core::result::CellValue;

use agentic_core::result::{ColumnSpec, TypedRowError, TypedRowStream, TypedValue};

use crate::connector::{
    ConnectorError, DatabaseConnector, ExecutionResult, ResultCap, SchemaColumnInfo, SchemaInfo,
    SchemaTableInfo, estimate_row_bytes, is_returning_statement, normalize_sql, plan_sql_script,
};

// Re-export Connection so callers / integration tests can construct connections
// without adding `duckdb` as a separate direct dependency.
pub use duckdb::Connection as DuckDbConnection;

// ── Load strategy & metadata ──────────────────────────────────────────────────

/// Controls whether a file is loaded lazily (view) or eagerly (temp table).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum LoadStrategy {
    /// `CREATE TEMP VIEW` — zero memory, re-reads the file on each query.
    ///
    /// Good for large files or one-shot queries.
    #[default]
    View,
    /// `CREATE TEMP TABLE AS SELECT *` — materialized in DuckDB's in-process
    /// memory.  Good for small files or repeated queries.
    Materialized,
}

/// Metadata about a table / view registered with the connector.
#[derive(Debug, Clone)]
pub struct TableInfo {
    pub name: String,
    /// `(column_name, data_type)` pairs returned by `DESCRIBE`.
    pub columns: Vec<(String, String)>,
    pub source: TableSource,
}

/// Where a [`TableInfo`] came from.
#[derive(Debug, Clone)]
pub enum TableSource {
    File {
        path: PathBuf,
        strategy: LoadStrategy,
    },
    /// Already existed in the DuckDB connection before we touched it.
    Native,
}

mod conversion;
mod dir_cache;
mod loading;
mod locked;
mod parse_check;
mod read_only;
mod sample;
mod schema;
mod session;

use conversion::{describe_type_to_typed, duckdb_to_cell_opt, duckdb_value_to_typed};
use loading::{Catalog, open_directory, open_in_memory, register_files};
use schema::{describe_query, detect_join_keys, parse_summarize_cell};
use session::{Session, Sessions};

pub struct DuckDbConnector {
    conn: Arc<Sessions>,
    /// Tables / views registered during construction.
    loaded_tables: Vec<TableInfo>,
    /// In-pod memory backstop for `execute_query_full` (see [`ResultCap`]).
    result_cap: ResultCap,
    /// `Some` for a connector built from a directory: the locked database
    /// `execute_query_read_only` runs on (see [`locked`]).
    read_only: Option<locked::ReadOnlyPath>,
}

// ── Constructors ──────────────────────────────────────────────────────────────

impl DuckDbConnector {
    /// Wrap an existing, already-configured DuckDB connection.
    ///
    /// Offers no read-only path: the connection may be shared with whoever
    /// opened it, so it cannot be locked (see [`locked`]).
    pub fn new(conn: Connection) -> Self {
        ensure_icu(&conn);
        Self::with_tables(conn, Vec::new())
    }

    fn with_tables(conn: Connection, loaded_tables: Vec<TableInfo>) -> Self {
        Self {
            conn: Arc::new(Sessions::new(Session::private(conn))),
            loaded_tables,
            result_cap: ResultCap::default(),
            read_only: None,
        }
    }

    /// With the read-only path over `dir`'s files.
    fn reading_from(mut self, dir: &Path, strategy: LoadStrategy) -> Self {
        self.read_only = Some(locked::ReadOnlyPath::new(dir, strategy));
        self
    }

    /// A clone of a cached directory database (see [`dir_cache`]).
    fn from_shared(
        conn: Connection,
        origin: Arc<dir_cache::SharedDb>,
        loaded_tables: Vec<TableInfo>,
    ) -> Self {
        let read_only = Some(locked::ReadOnlyPath::new(origin.dir(), origin.strategy()));
        Self {
            conn: Arc::new(Sessions::new(Session::shared(conn, origin))),
            loaded_tables,
            result_cap: ResultCap::default(),
            read_only,
        }
    }

    /// Override the [`ResultCap`] memory backstop. Primarily for tests that need
    /// to trip the guard on a small result.
    pub fn with_result_cap(mut self, cap: ResultCap) -> Self {
        self.result_cap = cap;
        self
    }

    /// Fresh in-memory DuckDB instance with no pre-loaded tables.
    pub fn in_memory() -> Result<Self, ConnectorError> {
        Ok(Self::new(open_in_memory()?))
    }

    /// Scan `dir` for `*.csv` and `*.parquet` files and register each as a
    /// view or temp table according to `strategy`.
    ///
    /// When two files share the same stem (e.g. `orders.csv` and
    /// `orders.parquet`) only the Parquet file is registered.
    ///
    /// Builds a fresh database every call. [`Self::from_directory_cached`]
    /// is the same connector without re-reading an unchanged directory.
    pub fn from_directory(dir: &Path, strategy: LoadStrategy) -> Result<Self, ConnectorError> {
        let (conn, loaded_tables) = open_directory(dir, strategy, Catalog::Temp)?;
        Ok(Self::with_tables(conn, loaded_tables).reading_from(dir, strategy))
    }

    /// [`Self::from_directory`], sharing one database per unchanged
    /// directory across the process: the first call registers and describes
    /// the files, later calls get their own connection to that database.
    /// Same tables, aliases and columns; a statement that may write moves the
    /// connector onto a private copy first. See [`dir_cache`].
    pub fn from_directory_cached(
        dir: &Path,
        strategy: LoadStrategy,
    ) -> Result<Self, ConnectorError> {
        dir_cache::global().connector(dir, strategy)
    }

    /// Register an explicit list of files, each with its own load strategy.
    ///
    /// # Example
    /// ```ignore
    /// DuckDbConnector::from_files(&[
    ///     (Path::new("small.csv"),     LoadStrategy::Materialized),
    ///     (Path::new("large.parquet"), LoadStrategy::View),
    /// ])
    /// ```
    pub fn from_files(files: &[(&Path, LoadStrategy)]) -> Result<Self, ConnectorError> {
        let conn = open_in_memory()?;
        let loaded_tables = register_files(&conn, files, Catalog::Temp)?;
        Ok(Self::with_tables(conn, loaded_tables))
    }

    /// Tables / views registered during construction.
    pub fn loaded_tables(&self) -> &[TableInfo] {
        &self.loaded_tables
    }
}

// ── Table naming ─────────────────────────────────────────────────────────────

/// Derive a DuckDB table name from a file stem that is safe to reference
/// unquoted. Spaces, hyphens, and other non-identifier characters are
/// collapsed to `_`, and a leading digit is prefixed with `_` so the name is
/// a valid bare identifier.
///
/// Downstream consumers (including LLM-generated `.view.yml` files) often
/// round-trip identifiers through normalization, so keeping table names to
/// `[A-Za-z_][A-Za-z0-9_]*` avoids silent rename drift.
fn normalize_table_name(stem: &str) -> String {
    let slug = slugify!(stem, separator = "_");
    if slug.is_empty() {
        return "unnamed".to_string();
    }
    if slug.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        format!("_{slug}")
    } else {
        slug
    }
}

/// Install + load the DuckDB ICU extension on `conn`.
///
/// Mirrors what oxy-core's DuckDB connector does on every connection
/// (`crates/core/src/connector/duckdb.rs`). ICU registers the
/// timezone-aware date/time function overloads; without it DuckDB's
/// binder fails to resolve `date_diff`/`date_trunc` calls whose part is a
/// bare string literal in some bind paths (e.g. a non-aggregated
/// `date_diff('day', <group_key>::DATE, CURRENT_DATE::DATE)` projected
/// over a populated `GROUP BY`), surfacing as a confusing
/// `No function matches … (STRING_LITERAL, DATE, DATE)` with an empty
/// candidate list. The agentic connector historically skipped this, so
/// the same SQL worked through the classic agent / SQL-IDE path (oxy-core
/// connector) but failed through automation `execute_sql` (this connector).
///
/// Best-effort: the extension is bundled with libduckdb-sys so this
/// succeeds offline, but a failure is logged rather than fatal so bare
/// in-memory connectors used in tests/sandboxes still construct.
fn ensure_icu(conn: &Connection) {
    if let Err(e) = conn
        .execute_batch("INSTALL icu;")
        .and_then(|()| conn.execute_batch("LOAD icu;"))
    {
        tracing::warn!(
            error = %e,
            "DuckDB ICU extension failed to load; date/time functions with a \
             string-literal part (date_diff/date_trunc) may fail to bind"
        );
    }
}

// ── DatabaseConnector impl ────────────────────────────────────────────────────

#[async_trait]
impl DatabaseConnector for DuckDbConnector {
    fn dialect(&self) -> crate::connector::SqlDialect {
        crate::connector::SqlDialect::DuckDb
    }

    #[cfg(feature = "arrow")]
    fn as_arrow(&self) -> Option<&dyn crate::connector::AsArrowConnector> {
        Some(self)
    }

    fn supports_read_only(&self) -> bool {
        self.read_only.is_some()
    }

    /// On the connector's locked database, never its session (see
    /// [`locked`]).
    #[tracing::instrument(
        target = "agentic_connector::query",
        name = "db.query",
        skip_all,
        err(level = "info", Display),
        fields(
            otel.name = %crate::telemetry::span_name(sql, "duckdb"),
            otel.kind = "client",
            db.system.name = "duckdb",
            db.operation.name = %crate::telemetry::operation_name(sql),
            db.query.text = %crate::telemetry::query_text(sql),
            oxy.db.method = "execute_query_read_only",
            oxy.db.sample_limit = sample_limit,
        )
    )]
    async fn execute_query_read_only(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        match &self.read_only {
            Some(path) => path.query(sql, sample_limit).await,
            None => Err(crate::connector::read_only_unsupported(self.dialect())),
        }
    }

    #[tracing::instrument(
        target = "agentic_connector::query",
        name = "db.query",
        skip_all,
        err(level = "info", Display),
        fields(
            otel.name = %crate::telemetry::span_name(sql, "duckdb"),
            otel.kind = "client",
            db.system.name = "duckdb",
            db.operation.name = %crate::telemetry::operation_name(sql),
            db.query.text = %crate::telemetry::query_text(sql),
            oxy.db.method = "execute_query",
            oxy.db.sample_limit = sample_limit,
        )
    )]
    async fn execute_query(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        // `execute_query` samples a single result set, so the rows/stats
        // path below wraps the user SQL in `CREATE TEMP TABLE _t AS (sql)`.
        // That wrap is only valid for one SELECT-family statement — a
        // multi-statement DDL/DML script (e.g. an automation `execute_sql`
        // setup file: `CREATE TABLE …; CREATE INDEX …; SELECT …`) would
        // produce `Parser Error: syntax error at or near "CREATE"`. Run
        // any leading statements for their side effects first, then sample
        // only the final statement.
        let script = plan_sql_script(sql);
        let turn = self.conn.turn_for(sql).await?;
        let conn = turn.session()?;

        for stmt in &script.prefix {
            conn.execute_batch(stmt)
                .map_err(|e| ConnectorError::query_failed(stmt.clone(), e.to_string()))?;
        }

        let sql = normalize_sql(&script.final_stmt);

        // The final statement doesn't return rows (DDL/DML such as a
        // trailing `CREATE INDEX` or `INSERT`): execute it for its side
        // effect and return a well-formed empty result instead of feeding
        // non-returning SQL into the sampling wrap.
        if !is_returning_statement(sql) {
            if !sql.is_empty() {
                conn.execute_batch(sql)
                    .map_err(|e| ConnectorError::query_failed(sql.to_string(), e.to_string()))?;
            }
            return Ok(ExecutionResult::empty());
        }

        sample::sample(&conn, sql, sample_limit)
    }

    async fn execute_statement(&self, sql: &str) -> Result<(), ConnectorError> {
        let sql = normalize_sql(sql);
        // Move the synchronous lock + `execute_batch` off the Tokio worker:
        // long warehouse CTAS calls flowing through here (e.g. preagg
        // build-plan rebuilds) would otherwise starve the runtime thread
        // that parked this task.
        let turn = self.conn.turn_for(sql).await?;
        let sql_owned = sql.to_string();
        spawn_blocking_with_hub(move || {
            let conn = turn.session()?;
            conn.execute_batch(&sql_owned)
                .map_err(|e| ConnectorError::query_failed(sql_owned.clone(), e.to_string()))
        })
        .await
        .map_err(|e| ConnectorError::ConnectionError(format!("blocking task panicked: {e}")))?
    }

    /// The first row of `sql`, mirroring `execute_query`: on the one session
    /// `turn_for(sql)` hands out, prefix statements (and a final statement
    /// that returns no rows) go through `execute_batch`, then the final
    /// statement is wrapped as `SELECT * FROM (\n{sql}\n) LIMIT 1` and read.
    #[tracing::instrument(
        target = "agentic_connector::query",
        name = "db.query",
        skip_all,
        err(level = "info", Display),
        fields(
            otel.name = %crate::telemetry::span_name(sql, "duckdb"),
            otel.kind = "client",
            db.system.name = "duckdb",
            db.operation.name = %crate::telemetry::operation_name(sql),
            db.query.text = %crate::telemetry::query_text(sql),
            oxy.db.method = "execute_query_first_row",
        )
    )]
    async fn execute_query_first_row(
        &self,
        sql: &str,
    ) -> Result<Option<Vec<TypedValue>>, ConnectorError> {
        let plan =
            crate::connector::plan_first_row(sql, |s| crate::connector::first_row_probe_sql(s, ""));
        let turn = self.conn.turn_for(sql).await?;
        let cap = self.result_cap;
        let stream = spawn_blocking_with_hub(move || {
            let conn = turn.session()?;
            for stmt in &plan.side_effects {
                conn.execute_batch(stmt)
                    .map_err(|e| ConnectorError::query_failed(stmt.clone(), e.to_string()))?;
            }
            plan.probe
                .map(|probe| read_typed(&conn, &probe, cap))
                .transpose()
        })
        .await
        .map_err(|e| ConnectorError::ConnectionError(format!("blocking task panicked: {e}")))??;
        match stream {
            Some(stream) => crate::connector::first_row_of(stream).await,
            None => Ok(None),
        }
    }

    #[tracing::instrument(
        target = "agentic_connector::query",
        name = "db.query",
        skip_all,
        err(level = "info", Display),
        fields(
            otel.name = %crate::telemetry::span_name(sql, "duckdb"),
            otel.kind = "client",
            db.system.name = "duckdb",
            db.operation.name = %crate::telemetry::operation_name(sql),
            db.query.text = %crate::telemetry::query_text(sql),
            oxy.db.method = "execute_query_full",
        )
    )]
    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        let sql = normalize_sql(sql);
        let turn = self.conn.turn_for(sql).await?;
        let sql_owned = sql.to_string();
        let cap = self.result_cap;
        // Move all blocking DuckDB work off the Tokio worker thread so concurrent
        // callers (e.g. join_all of 29 filter-count queries) can run in parallel
        // rather than serialising on the limited worker-thread pool.  Pattern is
        // identical to `execute_statement` above.
        spawn_blocking_with_hub(move || {
            let conn = turn.session()?;
            read_typed(&conn, &sql_owned, cap)
        })
        .await
        .map_err(|e| ConnectorError::ConnectionError(format!("blocking task panicked: {e}")))?
    }

    fn introspect_schema(&self) -> Result<SchemaInfo, ConnectorError> {
        let conn = self.conn.session()?;

        // ── 1. All non-system tables + views ──────────────────────────────────
        let table_rows: Vec<(String, String)> = conn
            .prepare(
                "SELECT table_schema, table_name \
                 FROM information_schema.tables \
                 WHERE table_schema NOT IN ('information_schema', 'pg_catalog') \
                   AND table_name NOT LIKE '_agentic_%' \
                 ORDER BY table_schema, table_name",
            )
            .map_err(|e| ConnectorError::Other(e.to_string()))?
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| ConnectorError::Other(e.to_string()))?
            .collect::<Result<_, duckdb::Error>>()
            .map_err(|e| ConnectorError::Other(e.to_string()))?;

        let mut tables: Vec<SchemaTableInfo> = Vec::with_capacity(table_rows.len());

        for (schema, table) in &table_rows {
            let qualified = format!("\"{schema}\".\"{table}\"");

            // ── 2. SUMMARIZE: one pass gives column_name, column_type, min, max.
            let summarize_rows: Vec<(String, String, Option<String>, Option<String>)> = conn
                .prepare(&format!("SUMMARIZE {qualified}"))
                .map_err(|e| ConnectorError::Other(e.to_string()))?
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(|e| ConnectorError::Other(e.to_string()))?
                .collect::<Result<_, duckdb::Error>>()
                .map_err(|e| ConnectorError::Other(e.to_string()))?;

            let col_count = summarize_rows.len();

            // ── 3. One sample query for all columns.
            let mut samples_by_idx: Vec<Vec<CellValue>> = vec![vec![]; col_count];
            if col_count > 0 {
                let sample_res = conn
                    .prepare(&format!("SELECT * FROM {qualified} LIMIT 5"))
                    .and_then(|mut stmt| {
                        stmt.query_map([], |row| {
                            (0..col_count)
                                .map(|i| row.get::<_, Value>(i))
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .map(|mapped| mapped.collect::<Result<Vec<_>, _>>())
                    });
                if let Ok(Ok(rows)) = sample_res {
                    for row_vals in rows {
                        for (i, v) in row_vals.into_iter().enumerate() {
                            if samples_by_idx[i].len() < 5
                                && let Some(cell) = duckdb_to_cell_opt(v)
                            {
                                samples_by_idx[i].push(cell);
                            }
                        }
                    }
                }
            }

            // ── 4. Build column infos from SUMMARIZE output ───────────────────
            let columns: Vec<SchemaColumnInfo> = summarize_rows
                .into_iter()
                .enumerate()
                .map(|(i, (col_name, col_type, min_str, max_str))| {
                    let min = min_str
                        .as_deref()
                        .and_then(|s| parse_summarize_cell(s, &col_type));
                    let max = max_str
                        .as_deref()
                        .and_then(|s| parse_summarize_cell(s, &col_type));
                    let sample_values = samples_by_idx.get(i).cloned().unwrap_or_default();
                    SchemaColumnInfo {
                        name: col_name,
                        data_type: col_type,
                        min,
                        max,
                        sample_values,
                    }
                })
                .collect();

            tables.push(SchemaTableInfo {
                name: table.clone(),
                columns,
            });
        }

        // ── 5. Auto-detect join keys ──────────────────────────────────────────
        let join_keys = detect_join_keys(&tables);

        Ok(SchemaInfo { tables, join_keys })
    }
}

// ── AsArrowConnector impl (feature = "arrow") ────────────────────────────────

#[cfg(feature = "arrow")]
#[async_trait]
impl crate::connector::AsArrowConnector for DuckDbConnector {
    async fn execute_query_arrow(
        &self,
        sql: &str,
    ) -> Result<crate::connector::ArrowQueryStream, ConnectorError> {
        let sql = normalize_sql(sql);
        let turn = self.conn.turn_for(sql).await?;
        let conn = turn.session()?;

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| ConnectorError::query_failed(sql.to_string(), e.to_string()))?;
        let arrow_iter = stmt
            .query_arrow([])
            .map_err(|e| ConnectorError::query_failed(sql.to_string(), e.to_string()))?;
        let schema = arrow_iter.get_schema();
        // Collect eagerly; `Statement` borrows from the connection and the
        // iterator cannot outlive the lock. This matches the eager collection
        // strategy used by `execute_query_full`.
        let batches: Vec<::arrow::array::RecordBatch> = arrow_iter.collect();
        drop(stmt);

        Ok(crate::connector::ArrowQueryStream {
            schema,
            batches: Box::pin(futures::stream::iter(batches.into_iter().map(Ok))),
        })
    }
}

/// Describe and run `sql` on `conn`, collecting at most the `ResultCap` of
/// typed rows. `sql` is one statement; the caller has already run any prefix.
fn read_typed(
    conn: &duckdb::Connection,
    sql_owned: &str,
    cap: ResultCap,
) -> Result<TypedRowStream, ConnectorError> {
    let sql_owned = sql_owned.to_string();
    // DESCRIBE resolves column names + types at the logical plan level —
    // no rows are fetched, no temp table needed.
    let described = describe_query(&conn, &sql_owned).map_err(|e| {
        ConnectorError::query_failed(format!("DESCRIBE ({sql_owned})"), e.to_string())
    })?;
    let columns: Vec<ColumnSpec> = described
        .iter()
        .map(|(name, ty)| ColumnSpec {
            name: name.clone(),
            data_type: describe_type_to_typed(ty),
        })
        .collect();
    let col_count = columns.len();
    let column_types: Vec<_> = columns.iter().map(|c| c.data_type.clone()).collect();

    let mut stmt = conn
        .prepare(&sql_owned)
        .map_err(|e| ConnectorError::query_failed(sql_owned.clone(), e.to_string()))?;

    let rows_iter = stmt
        .query_map([], |row| {
            let mut cells = Vec::with_capacity(col_count);
            for i in 0..col_count {
                let v: Value = row.get(i)?;
                cells.push(duckdb_value_to_typed(v, &column_types[i]));
            }
            Ok(cells)
        })
        .map_err(|e| ConnectorError::query_failed(sql_owned.clone(), e.to_string()))?;

    // Collect eagerly so we can release the Mutex and return a `'static`
    // stream — but stop at the ResultCap so an unbounded scan can't
    // materialize gigabytes in the pod before we hand back the stream.
    let mut rows: Vec<Result<Vec<TypedValue>, TypedRowError>> = Vec::new();
    let mut bytes: u64 = 0;
    let mut truncated = false;
    for row in rows_iter {
        match row {
            Ok(cells) => {
                bytes += estimate_row_bytes(&cells);
                rows.push(Ok(cells));
            }
            Err(e) => rows.push(Err(TypedRowError::DriverError(e.to_string()))),
        }
        if cap.exceeded(rows.len() as u64, bytes) {
            truncated = true;
            break;
        }
    }

    let stream = TypedRowStream::from_rows(columns, rows);
    Ok(if truncated {
        stream.with_truncation(Arc::new(AtomicBool::new(true)))
    } else {
        stream
    })
}

#[cfg(test)]
mod first_row_plan_tests {
    use crate::connector::{first_row_probe_sql, plan_first_row};

    #[test]
    fn a_two_statement_key_runs_the_prefix_and_wraps_the_last() {
        let plan = plan_first_row("SET threads = 1; SELECT max(ts) FROM t;", |s| {
            first_row_probe_sql(s, "")
        });
        assert_eq!(plan.side_effects, vec!["SET threads = 1"]);
        assert_eq!(
            plan.probe.as_deref(),
            Some("SELECT * FROM (\nSELECT max(ts) FROM t\n) LIMIT 1")
        );
    }

    #[test]
    fn a_no_rows_final_statement_is_executed_and_not_probed() {
        let plan = plan_first_row("SELECT 1; CREATE INDEX i ON t (c)", |s| s.to_string());
        assert_eq!(
            plan.side_effects,
            vec!["SELECT 1", "CREATE INDEX i ON t (c)"]
        );
        assert_eq!(plan.probe, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod span_capture {
        use std::collections::HashMap;
        use std::sync::{Arc, Mutex};

        use tracing::field::{Field, Visit};
        use tracing::span::{Attributes, Id};
        use tracing::{Event, Subscriber};
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::Context;
        use tracing_subscriber::registry::LookupSpan;

        pub type Fields = HashMap<String, String>;

        #[derive(Default, Clone)]
        pub struct Seen {
            pub spans: Arc<Mutex<Vec<(String, Fields)>>>,
            /// `(level, span it was inside, fields)`
            pub events: Arc<Mutex<Vec<(tracing::Level, String, Fields)>>>,
        }

        struct V(Fields);
        impl Visit for V {
            fn record_debug(&mut self, f: &Field, v: &dyn std::fmt::Debug) {
                self.0.insert(f.name().into(), format!("{v:?}"));
            }
            fn record_str(&mut self, f: &Field, v: &str) {
                self.0.insert(f.name().into(), v.into());
            }
        }

        impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Seen {
            fn on_new_span(&self, attrs: &Attributes<'_>, _id: &Id, _ctx: Context<'_, S>) {
                let mut v = V(Fields::new());
                attrs.record(&mut v);
                self.spans
                    .lock()
                    .unwrap()
                    .push((attrs.metadata().name().to_string(), v.0));
            }
            fn on_event(&self, e: &Event<'_>, ctx: Context<'_, S>) {
                let mut v = V(Fields::new());
                e.record(&mut v);
                let span = ctx
                    .event_span(e)
                    .map(|s| s.name().to_string())
                    .unwrap_or_default();
                self.events
                    .lock()
                    .unwrap()
                    .push((*e.metadata().level(), span, v.0));
            }
        }
    }

    /// Every statement is a `db.query` client span carrying the semconv
    /// attributes; a statement the warehouse rejects records its error inside
    /// that span at `info` (never `warn`+, which would also reach Sentry).
    #[tokio::test(flavor = "current_thread")]
    async fn a_query_is_a_db_span_and_a_rejected_one_records_its_error_at_info() {
        use tracing_subscriber::layer::SubscriberExt;
        let seen = span_capture::Seen::default();
        let _guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(seen.clone()));

        let conn = DuckDbConnector::new(Connection::open_in_memory().unwrap());
        conn.execute_query("-- revenue\nselect 42 as answer", 10)
            .await
            .unwrap();
        let _ = conn.execute_query("select * from no_such_table", 10).await;

        let spans = seen.spans.lock().unwrap().clone();
        let queries: Vec<_> = spans.iter().filter(|(n, _)| n == "db.query").collect();
        assert_eq!(queries.len(), 2, "{spans:?}");
        let f = &queries[0].1;
        assert_eq!(f["otel.name"], "SELECT duckdb");
        assert_eq!(f["otel.kind"], "client");
        assert_eq!(f["db.system.name"], "duckdb");
        assert_eq!(f["db.operation.name"], "SELECT");
        assert_eq!(f["db.query.text"], "-- revenue\nselect 42 as answer");
        assert_eq!(f["oxy.db.method"], "execute_query");
        assert_eq!(f["oxy.db.sample_limit"], "10");

        let events = seen.events.lock().unwrap().clone();
        let failure = events
            .iter()
            .find(|(_, span, fields)| span == "db.query" && fields.contains_key("error"))
            .unwrap_or_else(|| panic!("the rejected statement's error is on its span: {events:?}"));
        assert_eq!(failure.0, tracing::Level::INFO);
        assert!(failure.2["error"].contains("no_such_table"), "{failure:?}");
    }

    #[test]
    fn normalize_collapses_spaces_to_underscores() {
        assert_eq!(
            normalize_table_name("c20251018_lake_sonoma_100k copy"),
            "c20251018_lake_sonoma_100k_copy"
        );
        assert_eq!(normalize_table_name("my data file"), "my_data_file");
    }

    #[test]
    fn normalize_prefixes_leading_digit() {
        assert_eq!(
            normalize_table_name("20250816_tamalpa_headlands_50k"),
            "_20250816_tamalpa_headlands_50k"
        );
    }

    #[test]
    fn normalize_leaves_clean_names_intact() {
        assert_eq!(normalize_table_name("oxymart"), "oxymart");
        assert_eq!(normalize_table_name("orders_2024"), "orders_2024");
    }

    #[test]
    fn normalize_handles_hyphens_and_dots() {
        assert_eq!(normalize_table_name("my-table.v2"), "my_table_v2");
    }

    #[test]
    fn normalize_fallback_for_empty_slug() {
        assert_eq!(normalize_table_name(""), "unnamed");
        assert_eq!(normalize_table_name("---"), "unnamed");
    }
}
