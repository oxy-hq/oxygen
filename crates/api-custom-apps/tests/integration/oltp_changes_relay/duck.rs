//! An in-memory DuckDB as the relay's Airhouse ([`DuckSink`]), a sink that
//! dies once after its commit lands ([`FailsOnceAfterCommit`]), and one that
//! refuses its first views ([`RefusesViews`]).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use duckdb::Connection;
use duckdb::types::Value;
use oxy_api_custom_apps::oltp_changes::relay::sink::{ChangeSink, SinkError};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

/// Every workspace's Airhouse, as one in-memory DuckDB: the relay's DDL, its
/// one-transaction batches and its reads, and the tests' own reads.
pub(super) struct DuckSink(Mutex<Connection>);

impl DuckSink {
    pub(super) fn new() -> Self {
        Self(Mutex::new(
            Connection::open_in_memory().expect("an in-memory DuckDB"),
        ))
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every row of `sql`, each value as text.
    pub(super) fn read(&self, sql: &str) -> Vec<Vec<Option<String>>> {
        rows_as_text(&self.conn(), sql).unwrap_or_else(|e| panic!("{e}\n{sql}"))
    }

    /// Run `statements`, each its own commit.
    pub(super) fn exec(&self, statements: &[String]) {
        let conn = self.conn();
        for sql in statements {
            conn.execute_batch(sql)
                .unwrap_or_else(|e| panic!("{e}\n{sql}"));
        }
    }
}

/// A DuckDB failure as the relay sees one: DuckDB's own message, which says
/// "does not exist" for a missing schema or table, as Airhouse's does.
fn failed(e: duckdb::Error) -> SinkError {
    SinkError(e.to_string())
}

/// Every row of `sql`, each value as text: an integer as its digits, a
/// string as itself, any other value as DuckDB's Rust debug form. The relay
/// reads only `BIGINT`s and `VARCHAR`s; a test casts anything else in SQL.
fn rows_as_text(conn: &Connection, sql: &str) -> duckdb::Result<Vec<Vec<Option<String>>>> {
    let mut statement = conn.prepare(sql)?;
    let mut rows = statement.query([])?;
    let width = rows.as_ref().map_or(0, |s| s.column_count());
    let mut all = Vec::new();
    while let Some(row) = rows.next()? {
        let values = (0..width)
            .map(|i| row.get::<_, Value>(i).map(as_text))
            .collect::<duckdb::Result<Vec<_>>>()?;
        all.push(values);
    }
    Ok(all)
}

fn as_text(value: Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::Text(s) => Some(s),
        Value::Int(n) => Some(n.to_string()),
        Value::BigInt(n) => Some(n.to_string()),
        Value::HugeInt(n) => Some(n.to_string()),
        other => Some(format!("{other:?}")),
    }
}

#[async_trait]
impl ChangeSink for DuckSink {
    async fn has_airhouse(&self, _: &DatabaseConnection, _: Uuid) -> Result<bool, SinkError> {
        Ok(true)
    }

    async fn query(&self, _: Uuid, sql: &str) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        rows_as_text(&self.conn(), sql).map_err(failed)
    }

    async fn admin(&self, _: Uuid, statements: &[String]) -> Result<(), SinkError> {
        let conn = self.conn();
        for sql in statements {
            conn.execute_batch(sql).map_err(failed)?;
        }
        Ok(())
    }

    /// `BEGIN`, the statements and `COMMIT`; `ROLLBACK` on any failure.
    async fn write(&self, _: Uuid, statements: &[String]) -> Result<(), SinkError> {
        let conn = self.conn();
        conn.execute_batch("BEGIN").map_err(failed)?;
        let written = statements
            .iter()
            .try_for_each(|sql| conn.execute_batch(sql))
            .and_then(|()| conn.execute_batch("COMMIT"));
        if let Err(e) = written {
            // The original error is the one worth reporting.
            let _ = conn.execute_batch("ROLLBACK");
            return Err(failed(e));
        }
        Ok(())
    }
}

/// [`DuckSink`] whose first `write` commits, then fails: a relay that died
/// after the Airhouse commit and before it confirmed the batch in the store.
pub(super) struct FailsOnceAfterCommit {
    inner: Arc<DuckSink>,
    failed: AtomicBool,
}

/// What the first `write` reports after its commit landed.
pub(super) const LOST_AFTER_COMMIT: &str = "the connection dropped after COMMIT";

impl FailsOnceAfterCommit {
    pub(super) fn new(inner: Arc<DuckSink>) -> Self {
        Self {
            inner,
            failed: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ChangeSink for FailsOnceAfterCommit {
    async fn has_airhouse(
        &self,
        db: &DatabaseConnection,
        workspace: Uuid,
    ) -> Result<bool, SinkError> {
        self.inner.has_airhouse(db, workspace).await
    }

    async fn query(
        &self,
        workspace: Uuid,
        sql: &str,
    ) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        self.inner.query(workspace, sql).await
    }

    async fn admin(&self, workspace: Uuid, statements: &[String]) -> Result<(), SinkError> {
        self.inner.admin(workspace, statements).await
    }

    async fn write(&self, workspace: Uuid, statements: &[String]) -> Result<(), SinkError> {
        self.inner.write(workspace, statements).await?;
        if self.failed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        Err(SinkError(LOST_AFTER_COMMIT.to_owned()))
    }
}

/// [`DuckSink`] that refuses its first `refusals` current-state views, as
/// DuckLake might refuse the view's `QUALIFY`, and counts every view it was
/// sent.
pub(super) struct RefusesViews {
    inner: Arc<DuckSink>,
    refusals: Mutex<usize>,
    views: AtomicUsize,
}

/// What a refused view reports.
pub(super) const VIEW_REFUSED: &str = "Not implemented Error: QUALIFY in a view";

impl RefusesViews {
    pub(super) fn new(inner: Arc<DuckSink>, refusals: usize) -> Self {
        Self {
            inner,
            refusals: Mutex::new(refusals),
            views: AtomicUsize::new(0),
        }
    }

    /// How many view statements it was sent, refused or not.
    pub(super) fn views(&self) -> usize {
        self.views.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ChangeSink for RefusesViews {
    async fn has_airhouse(
        &self,
        db: &DatabaseConnection,
        workspace: Uuid,
    ) -> Result<bool, SinkError> {
        self.inner.has_airhouse(db, workspace).await
    }

    async fn query(
        &self,
        workspace: Uuid,
        sql: &str,
    ) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        self.inner.query(workspace, sql).await
    }

    async fn admin(&self, workspace: Uuid, statements: &[String]) -> Result<(), SinkError> {
        if statements
            .iter()
            .any(|s| s.contains("CREATE OR REPLACE VIEW"))
        {
            self.views.fetch_add(1, Ordering::SeqCst);
            let mut left = self.refusals.lock().unwrap_or_else(|e| e.into_inner());
            if *left > 0 {
                *left -= 1;
                return Err(SinkError(VIEW_REFUSED.to_owned()));
            }
        }
        self.inner.admin(workspace, statements).await
    }

    async fn write(&self, workspace: Uuid, statements: &[String]) -> Result<(), SinkError> {
        self.inner.write(workspace, statements).await
    }
}

/// [`DuckSink`] that dies at its first current-state view, as a relay that
/// died between a batch's confirm and its view would, then works.
pub(super) struct DiesAtFirstView {
    inner: Arc<DuckSink>,
    died: AtomicBool,
}

impl DiesAtFirstView {
    pub(super) fn new(inner: Arc<DuckSink>) -> Self {
        Self {
            inner,
            died: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl ChangeSink for DiesAtFirstView {
    async fn has_airhouse(
        &self,
        db: &DatabaseConnection,
        workspace: Uuid,
    ) -> Result<bool, SinkError> {
        self.inner.has_airhouse(db, workspace).await
    }

    async fn query(
        &self,
        workspace: Uuid,
        sql: &str,
    ) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        self.inner.query(workspace, sql).await
    }

    async fn admin(&self, workspace: Uuid, statements: &[String]) -> Result<(), SinkError> {
        let view = statements
            .iter()
            .any(|s| s.contains("CREATE OR REPLACE VIEW"));
        if view && !self.died.swap(true, Ordering::SeqCst) {
            panic!("the relay died before its view");
        }
        self.inner.admin(workspace, statements).await
    }

    async fn write(&self, workspace: Uuid, statements: &[String]) -> Result<(), SinkError> {
        self.inner.write(workspace, statements).await
    }
}
