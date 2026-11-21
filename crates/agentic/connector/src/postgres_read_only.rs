//! `execute_query_read_only` for Postgres (and Redshift, the same connector):
//! model-written SQL in a transaction the server keeps read-only.
//!
//! **The transaction.** On the connector's one client, under its mutex for
//! the whole exchange, so no other query interleaves: `BEGIN READ ONLY`, a
//! check that the server really is read-only now (a pgwire server that
//! accepted and ignored it would otherwise fail open), the statement, then
//! `ROLLBACK` — after an error too. If the exchange stops before `ROLLBACK`
//! succeeded (an error, a future dropped mid-statement), [`ReadOnlyTxn`]
//! drops the client: the server aborts the transaction when the connection
//! closes, and the next query reconnects instead of running inside it.
//!
//! **Who may run it.** In the same transaction, before the statement, the
//! login role is refused if it, or any role it is a member of, is a
//! superuser, is `pg_write_server_files`, `pg_read_server_files` or
//! `pg_execute_server_program`, or may execute `lo_export(oid, text)` or a
//! server-file function held outside those roles (`FILE_FUNCTIONS`: the
//! file read/list functions, and adminpack's where installed). Each can reach the server's
//! files or programs, which no read-only transaction stops (`SELECT
//! lo_export(…)` changes no database state). Membership counts because the
//! statement can switch role itself (`set_config('role', …, true)`, then SQL
//! built at run time such as `query_to_xml`). Give the agent a
//! least-privilege login role that cannot switch to any privileged role:
//! `SELECT` on what it reads, nothing else.
//!
//! **Before a run.** [`PostgresConnector::read_only_probe`] runs the same
//! transaction and check with no statement, so a react run on a role that
//! would refuse every query refuses at its start. Each query still checks.
//!
//! **Bounds.** Right after `BEGIN READ ONLY`, `SET LOCAL statement_timeout`
//! and `lock_timeout` ([`STATEMENT_TIMEOUT`], [`LOCK_TIMEOUT`]) bound how long
//! the agent's statement may run or wait; after `ROLLBACK`,
//! `pg_advisory_unlock_all()` releases any session-level advisory lock it
//! took (a transaction-level one ends with the rollback).
//!
//! **What remains** (none of it a write to the database, all of it bounded by
//! the role's privileges): `pg_terminate_backend` / `pg_cancel_backend`
//! against other backends of the same role, `pg_notify`, `dblink` or another
//! extension that opens its own connection where one is installed, and the
//! side effects of any `SECURITY DEFINER` function the role can execute.
//!
//! **The statement.** The agent's SQL reaches the server only through the
//! extended protocol, which refuses `SELECT 1; DELETE …` outright: once alone,
//! prepared (parsed, never executed) for its columns, then inside one
//! [`wrap`]ped statement that is executed. A statement with no result columns
//! (`COPY … TO`, DDL, DML without `RETURNING`) is refused without running: a
//! read-only transaction does not stop `COPY … TO` a server file, and inside
//! the wrapper only a query parses.
//!
//! **The result.** One execution answers `execute_query`'s whole contract:
//! the agent's SQL is a CTE read twice (Postgres materializes it), once for
//! the row count and per-column stats, once for the `::TEXT` sample, the
//! expressions `execute_query` runs against its temp table and read back
//! with the same [`pg_text_to_cell`]. One difference: `mean` and `std_dev`
//! come only from a numeric column, since a failed `::DOUBLE PRECISION` cast
//! would fail the whole statement; `execute_query` also averages a text
//! column whose every value is a number.

use std::time::Duration;

use agentic_core::result::{QueryResult, QueryRow};
use tokio::sync::MutexGuard;
use tokio_postgres::{Client, Row};

use super::{PostgresConnector, ensure_client_connected, pg_text_to_cell};
use crate::connector::{
    ColumnStats, ConnectorError, ExecutionResult, QueryFailedDetails, ResultSummary, normalize_sql,
};
use crate::pg_error::{pg_error_message, pg_query_failed};

/// What precedes the agent's SQL in [`wrap`]: a position the server reports
/// is shifted back by its length. The newline after the SQL keeps a trailing
/// `--` comment from swallowing the closing parenthesis.
const PREFIX: &str = "WITH __oxy_q AS (\n";

/// How long the agent's statement may run.
pub const STATEMENT_TIMEOUT: Duration = Duration::from_secs(60);
/// How long the agent's statement may wait for a lock.
pub const LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// The two bounds a read-only transaction sets.
#[derive(Debug, Clone, Copy)]
pub(super) struct Timeouts {
    pub(super) statement: Duration,
    pub(super) lock: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            statement: STATEMENT_TIMEOUT,
            lock: LOCK_TIMEOUT,
        }
    }
}

/// The transaction's mode, and whether any role the login role can reach —
/// itself, or one it could `SET ROLE` to (`set_config('role', …, true)` from
/// inside the agent's own statement) — can reach a server file or program: a
/// superuser, one of the three server-file/program roles, or a holder of
/// `EXECUTE` on `lo_export` or on any function in [`FILE_FUNCTIONS`].
/// `MEMBER` is any membership, whatever its `INHERIT` or `SET` option, so it
/// covers every role `SET ROLE` can reach. On a server without the core
/// catalogs or `lo_export` the query fails, and so does the read; a listed
/// function that does not exist on this server (`to_regprocedure` is `NULL`:
/// adminpack, an overload of another version) counts as no grant.
const CHECK: &str = "SELECT current_setting('transaction_read_only'), \
    EXISTS (SELECT 1 FROM pg_roles r \
        WHERE pg_has_role(session_user, r.oid, 'MEMBER') \
        AND (r.rolsuper \
            OR r.rolname IN ('pg_write_server_files', 'pg_read_server_files', \
                'pg_execute_server_program') \
            OR has_function_privilege(r.oid, 'lo_export(oid, text)', 'EXECUTE') \
            OR EXISTS (SELECT 1 FROM unnest($1::text[]) AS f(sig) \
                WHERE to_regprocedure(f.sig) IS NOT NULL \
                AND has_function_privilege(r.oid, to_regprocedure(f.sig)::oid, 'EXECUTE'))))";

/// Functions that read, list, write, rename or delete server files, by
/// signature, every overload: core `pg_read_file`, `pg_read_binary_file`,
/// `pg_ls_dir`, `pg_stat_file` and the `pg_ls_*dir` family (only superusers,
/// `pg_read_server_files` or `pg_monitor` hold these by default), and
/// adminpack's file functions (PG 17 removed adminpack). A role granted
/// `EXECUTE` on any of them directly is refused like a member of the role
/// that holds them.
const FILE_FUNCTIONS: &[&str] = &[
    "pg_catalog.pg_read_file(text)",
    "pg_catalog.pg_read_file(text, boolean)",
    "pg_catalog.pg_read_file(text, bigint, bigint)",
    "pg_catalog.pg_read_file(text, bigint, bigint, boolean)",
    "pg_catalog.pg_read_binary_file(text)",
    "pg_catalog.pg_read_binary_file(text, boolean)",
    "pg_catalog.pg_read_binary_file(text, bigint, bigint)",
    "pg_catalog.pg_read_binary_file(text, bigint, bigint, boolean)",
    "pg_catalog.pg_ls_dir(text)",
    "pg_catalog.pg_ls_dir(text, boolean, boolean)",
    "pg_catalog.pg_stat_file(text)",
    "pg_catalog.pg_stat_file(text, boolean)",
    "pg_catalog.pg_ls_logdir()",
    "pg_catalog.pg_ls_waldir()",
    "pg_catalog.pg_ls_tmpdir()",
    "pg_catalog.pg_ls_tmpdir(oid)",
    "pg_catalog.pg_ls_archive_statusdir()",
    "pg_catalog.pg_ls_logicalsnapdir()",
    "pg_catalog.pg_ls_logicalmapdir()",
    "pg_catalog.pg_ls_replslotdir(text)",
    "pg_catalog.pg_file_write(text, text, boolean)",
    "pg_catalog.pg_file_rename(text, text, text)",
    "pg_catalog.pg_file_rename(text, text)",
    "pg_catalog.pg_file_unlink(text)",
];

/// Postgres types `AVG(col::DOUBLE PRECISION)` always accepts.
const NUMERIC: &[&str] = &["int2", "int4", "int8", "float4", "float8", "numeric"];

/// Expressions per column in the stats CTE.
const STATS_PER_COLUMN: usize = 6;

/// The refusal of a Redshift connector's read-only path (see
/// `PostgresConnector::redshift`).
pub(super) fn redshift_unsupported() -> ConnectorError {
    ConnectorError::Other("read-only execution is not supported by Redshift".into())
}

impl PostgresConnector {
    pub(super) async fn read_only_query(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        let sql = normalize_sql(sql);
        let txn = self.begin_read_only().await?;
        let result = sample(txn.client(), sql, sample_limit).await;
        let rolled_back = txn.rollback().await;
        let result = result?;
        rolled_back?;
        Ok(result)
    }

    /// The read-only transaction and its role check, with no statement:
    /// `Err` where every query would be refused.
    pub(super) async fn read_only_probe(&self) -> Result<(), ConnectorError> {
        self.begin_read_only().await?.rollback().await
    }

    /// Connects if needed and begins a checked read-only transaction.
    async fn begin_read_only(&self) -> Result<ReadOnlyTxn<'_>, ConnectorError> {
        let mut guard = self.client.lock().await;
        ensure_client_connected(
            &self.config,
            self.verify_tls,
            &mut guard,
            &self.cached_schema,
            &self.schema_error,
        )
        .await?;
        let mut txn = ReadOnlyTxn {
            guard,
            settled: false,
        };
        txn.begin(self.read_only_timeouts).await?;
        Ok(txn)
    }
}

/// The connector's client inside `BEGIN READ ONLY` … `ROLLBACK`, holding its
/// mutex. Dropped before `ROLLBACK` succeeded, it drops the client too.
struct ReadOnlyTxn<'a> {
    guard: MutexGuard<'a, Option<Client>>,
    settled: bool,
}

impl ReadOnlyTxn<'_> {
    fn client(&self) -> &Client {
        self.guard
            .as_ref()
            .expect("connected before the transaction began")
    }

    async fn begin(&mut self, timeouts: Timeouts) -> Result<(), ConnectorError> {
        let client = self.client();
        let begin = format!(
            "BEGIN READ ONLY; SET LOCAL statement_timeout = {}; SET LOCAL lock_timeout = {}",
            timeouts.statement.as_millis(),
            timeouts.lock.as_millis()
        );
        client
            .batch_execute(&begin)
            .await
            .map_err(|e| ConnectorError::QueryFailed(pg_query_failed(&begin, &e)))?;
        let row = client
            .query_one(CHECK, &[&FILE_FUNCTIONS])
            .await
            .map_err(|e| ConnectorError::QueryFailed(pg_query_failed(CHECK, &e)))?;
        let mode: String = row.get(0);
        if mode != "on" {
            return Err(ConnectorError::Other(format!(
                "the server did not start a read-only transaction (transaction_read_only = \
                 {mode}), so the query was not run"
            )));
        }
        let writes_files: bool = row.get(1);
        if writes_files {
            return Err(ConnectorError::Other(
                "read-only queries refuse this connection's role: it, or a role it can switch \
                 to, can reach server files or programs (superuser, pg_write_server_files, \
                 pg_read_server_files, pg_execute_server_program, or EXECUTE on lo_export or \
                 a server-file function), \
                 which a read-only transaction does not stop; connect as a least-privilege role"
                    .into(),
            ));
        }
        Ok(())
    }

    async fn rollback(mut self) -> Result<(), ConnectorError> {
        let end = "ROLLBACK; SELECT pg_advisory_unlock_all()";
        self.client().batch_execute(end).await.map_err(|e| {
            ConnectorError::ConnectionError(format!(
                "ROLLBACK after a read-only query failed: {}",
                pg_error_message(&e)
            ))
        })?;
        self.settled = true;
        Ok(())
    }
}

impl Drop for ReadOnlyTxn<'_> {
    fn drop(&mut self) {
        if !self.settled {
            // The connection may still be inside the transaction: close it.
            self.guard.take();
        }
    }
}

/// A result column: its name and `pg_type.typname`.
struct Column {
    name: String,
    typname: String,
}

impl Column {
    fn quoted(&self) -> String {
        format!("\"{}\"", self.name.replace('"', "\"\""))
    }
}

async fn sample(
    client: &Client,
    sql: &str,
    sample_limit: u64,
) -> Result<ExecutionResult, ConnectorError> {
    let described = client
        .prepare(sql)
        .await
        .map_err(|e| ConnectorError::QueryFailed(pg_query_failed(sql, &e)))?;
    let columns: Vec<Column> = described
        .columns()
        .iter()
        .map(|c| Column {
            name: c.name().to_string(),
            typname: c.type_().name().to_string(),
        })
        .collect();
    drop(described);
    if columns.is_empty() {
        return Err(ConnectorError::query_failed(
            sql,
            "a read-only query must return rows; this statement returns none and was not run",
        ));
    }
    let rows = client
        .query(wrap(sql, &columns, sample_limit).as_str(), &[])
        .await
        .map_err(|e| ConnectorError::QueryFailed(unwrapped(pg_query_failed(sql, &e), sql)))?;
    Ok(read_back(&columns, &rows))
}

/// One statement: the stats row joined to each sample row, in the agent's
/// row order. The stats row stands alone when the result is empty.
fn wrap(sql: &str, columns: &[Column], sample_limit: u64) -> String {
    let stats: Vec<String> = columns
        .iter()
        .map(|c| {
            let q = c.quoted();
            let (mean, std_dev) = if NUMERIC.contains(&c.typname.as_str()) {
                (
                    format!("AVG({q}::DOUBLE PRECISION)"),
                    format!("STDDEV_POP({q}::DOUBLE PRECISION)"),
                )
            } else {
                let none = "NULL::DOUBLE PRECISION".to_string();
                (none.clone(), none)
            };
            format!(
                "COUNT(*) - COUNT({q}), COUNT(DISTINCT {q}), MIN({q})::TEXT, MAX({q})::TEXT, \
                 {mean}, {std_dev}"
            )
        })
        .collect();
    let cells: Vec<String> = columns
        .iter()
        .map(|c| format!("{}::TEXT", c.quoted()))
        .collect();
    format!(
        "{PREFIX}{sql}\n),\n\
         __oxy_s AS (SELECT COUNT(*), {stats} FROM __oxy_q),\n\
         __oxy_r AS (SELECT row_number() OVER () AS __oxy_rn, {cells} FROM __oxy_q \
         LIMIT {sample_limit})\n\
         SELECT __oxy_s.*, __oxy_r.* FROM __oxy_s LEFT JOIN __oxy_r ON TRUE \
         ORDER BY __oxy_r.__oxy_rn",
        stats = stats.join(", "),
        cells = cells.join(", "),
    )
}

/// A failure of the wrapped statement, pointing into the agent's SQL: a
/// position inside it is shifted back, one inside the wrapper dropped.
fn unwrapped(mut details: QueryFailedDetails, sql: &str) -> QueryFailedDetails {
    let start = PREFIX.chars().count() as u32;
    let len = sql.chars().count() as u32;
    details.position = details
        .position
        .filter(|p| *p > start && *p <= start + len)
        .map(|p| p - start);
    details
}

fn read_back(columns: &[Column], rows: &[Row]) -> ExecutionResult {
    let first = &rows[0];
    let total_row_count = first.get::<_, i64>(0) as u64;
    let col_stats = columns
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let at = 1 + STATS_PER_COLUMN * i;
            ColumnStats {
                name: c.name.clone(),
                data_type: Some(c.typname.clone()),
                null_count: first.get::<_, i64>(at) as u64,
                distinct_count: Some(first.get::<_, i64>(at + 1) as u64),
                min: Some(pg_text_to_cell(first.get(at + 2))),
                max: Some(pg_text_to_cell(first.get(at + 3))),
                mean: first.get(at + 4),
                std_dev: first.get(at + 5),
            }
        })
        .collect();
    let rn = 1 + STATS_PER_COLUMN * columns.len();
    let sample_rows: Vec<QueryRow> = rows
        .iter()
        .filter(|r| r.get::<_, Option<i64>>(rn).is_some())
        .map(|r| {
            QueryRow(
                (0..columns.len())
                    .map(|i| pg_text_to_cell(r.get(rn + 1 + i)))
                    .collect(),
            )
        })
        .collect();
    let truncated = (sample_rows.len() as u64) < total_row_count;
    ExecutionResult {
        result: QueryResult {
            columns: columns.iter().map(|c| c.name.clone()).collect(),
            rows: sample_rows,
            total_row_count,
            truncated,
        },
        summary: ResultSummary {
            row_count: total_row_count,
            columns: col_stats,
        },
    }
}
