//! `execute_query_read_only` for DuckDB: model-written SQL on a database of
//! its own that cannot reach files, in a transaction that is always rolled
//! back.
//!
//! **Why a database of its own.** DuckDB's file-access settings are
//! instance-wide (`duckdb_settings()` scopes them `GLOBAL`), and turning
//! external access off cannot be undone while the database runs. Every
//! connection a connector might hold is shared with someone: a
//! [`super::dir_cache`] clone with every run reading that directory, a
//! connection handed to [`super::DuckDbConnector::new`] with whoever opened it
//! (the app's local DuckDB is a process-wide pool). Locking one of those would
//! lock them all. So a connector built from a directory builds a second,
//! private database over the same files the first time a read-only query
//! arrives ([`ReadOnlyPath`]), and locks that; a connector over a handed-in
//! connection has nothing to rebuild from and offers no read-only path.
//!
//! **The lock** ([`lock`]), applied after the files are registered:
//! extension autoload and autoinstall off, `allowed_paths` lists exactly
//! the files behind views (a materialized table needs none),
//! `enable_external_access = false` refuses every other path —
//! `COPY … TO`, `ATTACH`, `read_csv` of another file, `INSTALL`, `LOAD` — and
//! `lock_configuration = true` refuses `SET`/`RESET` of any of it.
//! `allowed_directories` would be wider than it looks: it lets `COPY … TO`
//! and `ATTACH` write into the directory.
//!
//! **What the lock alone does not stop:** a listed path is allowed for
//! writing as well as reading. A plain `COPY … TO` one of the agent's own
//! view files is still refused by the engine, because `COPY` first writes a
//! temp file beside it, which is not listed; `COPY … TO` it with
//! `USE_TMP_FILE false` would overwrite it, and is refused only by the
//! statement check below (DuckDB's parser reports a `COPY`, not a `SELECT`).
//! The lib tests pin that boundary (`copy_onto_a_view_file`).
//!
//! **Each statement** must be one statement by the connector's splitter,
//! a read by [`super::read_only`]'s classifier (which also refuses the
//! instance-level table functions such as `enable_logging()`), and exactly
//! one `SELECT` by DuckDB's own parser ([`Shadow::one_select`]). That check
//! binds the statement, and binding opens the files and URLs it names, so it
//! runs on a shadow locked the same way before its first bind
//! ([`Shadow::lock_down`]), never on the FSM's unlocked one; then it runs
//! through the shared sampler ([`super::sample`]) between `BEGIN TRANSACTION`
//! and `ROLLBACK`, so a write to the catalog that got past every check is
//! still undone.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agentic_core::hub_task::spawn_blocking_with_hub;
use duckdb::Connection;

use super::loading::{Catalog, open_directory};
use super::parse_check::Shadow;
use super::{LoadStrategy, TableInfo, TableSource, read_only, sample};
use crate::connector::{ConnectorError, ExecutionResult, normalize_sql, split_sql_statements};
use crate::string_literal::StringLiteral;

/// The read-only path of a connector built from a directory: what to build
/// the locked database from, and the database once built.
pub(super) struct ReadOnlyPath {
    dir: PathBuf,
    strategy: LoadStrategy,
    db: tokio::sync::OnceCell<Arc<LockedDb>>,
}

impl ReadOnlyPath {
    pub(super) fn new(dir: &Path, strategy: LoadStrategy) -> Self {
        Self {
            dir: dir.to_path_buf(),
            strategy,
            db: tokio::sync::OnceCell::new(),
        }
    }

    /// Run `sql` on the locked database, building it on first use. Both the
    /// build and the query run on the blocking pool.
    pub(super) async fn query(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        let db = self
            .db
            .get_or_try_init(|| {
                let (dir, strategy) = (self.dir.clone(), self.strategy);
                blocking(move || LockedDb::build(&dir, strategy).map(Arc::new))
            })
            .await?
            .clone();
        let sql = sql.to_string();
        blocking(move || db.query(&sql, sample_limit)).await
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ConnectorError> + Send + 'static,
) -> Result<T, ConnectorError> {
    spawn_blocking_with_hub(f)
        .await
        .map_err(|e| ConnectorError::ConnectionError(format!("blocking task panicked: {e}")))?
}

/// A private database over a directory's files, locked by [`lock`].
pub(super) struct LockedDb {
    /// `None` once a `ROLLBACK` failed: a connection that may still be inside
    /// a transaction is never used again.
    conn: Mutex<Option<Connection>>,
    shadow: Shadow,
}

impl LockedDb {
    pub(super) fn build(dir: &Path, strategy: LoadStrategy) -> Result<Self, ConnectorError> {
        let (conn, tables) = open_directory(dir, strategy, Catalog::Temp)?;
        lock(&conn, &tables)?;
        // Locked before its first bind: binding a query that names a file or
        // a URL reads it, on whatever database the check runs on.
        let shadow = Shadow::build(&tables, dir)
            .filter(Shadow::lock_down)
            .ok_or_else(|| {
                ConnectorError::ConnectionError(
                    "DuckDB: could not build and lock the parse check a read-only query needs"
                        .into(),
                )
            })?;
        Ok(Self {
            conn: Mutex::new(Some(conn)),
            shadow,
        })
    }

    /// Admit `sql` ([`admit`]), then sample it in a rolled-back transaction.
    /// Blocking.
    pub(super) fn query(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        let sql = normalize_sql(sql);
        admit(sql, &self.shadow)?;
        let mut guard = self
            .conn
            .lock()
            .map_err(|e| ConnectorError::ConnectionError(format!("mutex poisoned: {e}")))?;
        let conn = guard.as_ref().ok_or_else(|| {
            ConnectorError::ConnectionError(
                "the read-only DuckDB database closed after a failed ROLLBACK".into(),
            )
        })?;
        let (result, rolled_back) = rolled_back(conn, |c| sample::sample(c, sql, sample_limit));
        if let Err(e) = rolled_back {
            *guard = None;
            result?;
            return Err(e);
        }
        result
    }

    /// The locked connection, for a test that bypasses [`admit`].
    #[cfg(test)]
    pub(super) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(self.conn.lock().unwrap().as_ref().unwrap())
    }
}

/// The checks a statement passes before it runs: one statement, a read, and
/// one `SELECT` to DuckDB's parser.
fn admit(sql: &str, shadow: &Shadow) -> Result<(), ConnectorError> {
    let refuse = |why: &str| Err(ConnectorError::query_failed(sql, why));
    if split_sql_statements(sql).len() != 1 {
        return refuse("a read-only query is exactly one statement");
    }
    if !read_only::is_read_only(sql) {
        return refuse("a read-only query cannot write or change the database");
    }
    if let Err(why) = shadow.one_select(sql) {
        return refuse(&format!(
            "DuckDB does not admit this as a single SELECT: {why}"
        ));
    }
    Ok(())
}

/// Run `f` between `BEGIN TRANSACTION` and `ROLLBACK`: its result, and
/// whether the rollback succeeded.
pub(super) fn rolled_back<T>(
    conn: &Connection,
    f: impl FnOnce(&Connection) -> Result<T, ConnectorError>,
) -> (Result<T, ConnectorError>, Result<(), ConnectorError>) {
    if let Err(e) = conn.execute_batch("BEGIN TRANSACTION") {
        let failed = ConnectorError::query_failed("BEGIN TRANSACTION", e.to_string());
        return (Err(failed), Ok(()));
    }
    let result = f(conn);
    let rolled_back = conn.execute_batch("ROLLBACK").map_err(|e| {
        ConnectorError::ConnectionError(format!("ROLLBACK after a read-only query failed: {e}"))
    });
    (result, rolled_back)
}

/// Close `conn`'s database to every path but the files its views read, and
/// lock its configuration. Fails rather than leave it half-locked.
pub(super) fn lock(conn: &Connection, tables: &[TableInfo]) -> Result<(), ConnectorError> {
    let read: Vec<String> = tables
        .iter()
        .filter_map(|t| match &t.source {
            TableSource::File {
                path,
                strategy: LoadStrategy::View,
            } => Some(StringLiteral::Standard.quote(&path.display().to_string())),
            _ => None,
        })
        .collect();
    let mut statements = vec![
        "SET autoload_known_extensions = false".to_string(),
        "SET autoinstall_known_extensions = false".to_string(),
    ];
    if !read.is_empty() {
        statements.push(format!("SET allowed_paths = [{}]", read.join(", ")));
    }
    statements.push("SET enable_external_access = false".to_string());
    statements.push("SET lock_configuration = true".to_string());
    for stmt in statements {
        conn.execute_batch(&stmt).map_err(|e| {
            ConnectorError::ConnectionError(format!("DuckDB: could not lock ({stmt}): {e}"))
        })?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "locked_tests.rs"]
mod tests;
