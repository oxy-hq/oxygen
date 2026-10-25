//! Test stand-ins for the usage tests of this crate and of the crates that read
//! usage (`oxy-api-custom-apps`'s Activity tab, `oxy-app`'s weekly report).
//! Behind the `test-fixtures` feature; never part of a release build.
//!
//! * [`FakeAirhouse`] is a [`UsageSink`] over one in-memory DuckDB per
//!   workspace. DuckLake is DuckDB underneath, so the statements the sink sends
//!   (`insert_sql`, the reads in `usage_reads`) run unchanged — the only thing
//!   missing is DuckLake itself, which is why the table's DDL is checked for
//!   keys, indexes and constraints separately.
//! * [`history`] is a small, fixed usage history for one app: views and events
//!   of both environments, of this week and the week before, with the NULL
//!   roles, the repeated sessions and the reserved `oxy-error` events that the
//!   readers' queries have to get right.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

mod history;
pub use history::{Cast, History, history};

use crate::usage_airhouse::{
    CREATE_TABLE, SinkError, SqlRows, TABLE, UsageFact, UsageSink, insert_sql,
};

/// A workspace's Airhouse, in memory. A workspace has one only once
/// [`with_airhouse`](Self::with_airhouse) said so; any other is "no Airhouse".
#[derive(Default)]
pub struct FakeAirhouse {
    workspaces: Mutex<HashMap<Uuid, Arc<Mutex<duckdb::Connection>>>>,
    queries: AtomicU64,
    appends: AtomicU64,
    /// Appends that succeed before every later one fails; `u64::MAX` = never.
    appends_before_failure: AtomicU64,
    fail_queries: std::sync::atomic::AtomicBool,
    query_delay_ms: AtomicU64,
}

impl FakeAirhouse {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            appends_before_failure: AtomicU64::new(u64::MAX),
            ..Self::default()
        })
    }

    /// Give `workspace_id` an Airhouse (an empty one: the table is created by
    /// the first `prepare`, as in production).
    pub fn with_airhouse(self: &Arc<Self>, workspace_id: Uuid) -> &Arc<Self> {
        self.workspaces.lock().unwrap().insert(
            workspace_id,
            Arc::new(Mutex::new(
                duckdb::Connection::open_in_memory().expect("in-memory DuckDB"),
            )),
        );
        self
    }

    /// Make this the process's sink.
    pub fn install(self: &Arc<Self>) {
        crate::usage_airhouse::install_sink(self.clone());
    }

    /// Let `n` more appends through, then fail every one after.
    pub fn fail_appends_after(&self, n: u64) {
        self.appends_before_failure
            .store(self.appends.load(Ordering::SeqCst) + n, Ordering::SeqCst);
    }

    pub fn stop_failing_appends(&self) {
        self.appends_before_failure
            .store(u64::MAX, Ordering::SeqCst);
    }

    /// Make every read fail, as an unreachable Airhouse does.
    /// Make every read wait this long before it answers, as a slow Airhouse.
    pub fn delay_queries(&self, delay: std::time::Duration) {
        self.query_delay_ms
            .store(delay.as_millis() as u64, Ordering::SeqCst);
    }

    pub fn fail_queries(&self, fail: bool) {
        self.fail_queries.store(fail, Ordering::SeqCst);
    }

    /// Reads served since creation.
    pub fn queries(&self) -> u64 {
        self.queries.load(Ordering::SeqCst)
    }

    /// Appends that reached the table (failed ones are not counted).
    pub fn appends(&self) -> u64 {
        self.appends.load(Ordering::SeqCst)
    }

    /// Rows in `workspace_id`'s table, duplicates included; `0` before the
    /// table exists.
    pub fn row_count(&self, workspace_id: Uuid) -> i64 {
        self.scalar(workspace_id, &format!("SELECT count(*) FROM {TABLE}"))
            .unwrap_or(0)
    }

    /// Distinct event ids in `workspace_id`'s table.
    pub fn distinct_ids(&self, workspace_id: Uuid) -> i64 {
        self.scalar(
            workspace_id,
            &format!("SELECT count(DISTINCT event_id) FROM {TABLE}"),
        )
        .unwrap_or(0)
    }

    /// Run `sql` straight on `workspace_id`'s database, for a test to arrange
    /// a row or break the table.
    pub fn exec(&self, workspace_id: Uuid, sql: &str) {
        let conn = self.connection(workspace_id).expect("an Airhouse");
        conn.lock()
            .unwrap()
            .execute_batch(sql)
            .unwrap_or_else(|e| panic!("{e}: {sql}"));
    }

    fn connection(&self, workspace_id: Uuid) -> Option<Arc<Mutex<duckdb::Connection>>> {
        self.workspaces.lock().unwrap().get(&workspace_id).cloned()
    }

    fn scalar(&self, workspace_id: Uuid, sql: &str) -> Option<i64> {
        let conn = self.connection(workspace_id)?;
        let conn = conn.lock().unwrap();
        conn.query_row(sql, [], |row| row.get::<_, i64>(0)).ok()
    }
}

#[async_trait]
impl UsageSink for FakeAirhouse {
    fn enabled(&self) -> bool {
        true
    }

    async fn append(
        &self,
        _db: &DatabaseConnection,
        workspace_id: Uuid,
        facts: &[UsageFact],
    ) -> Result<(), SinkError> {
        let conn = self
            .connection(workspace_id)
            .ok_or(SinkError::NotConfigured)?;
        let Some(sql) = insert_sql(facts) else {
            return Ok(());
        };
        if self.appends.load(Ordering::SeqCst) >= self.appends_before_failure.load(Ordering::SeqCst)
        {
            return Err(SinkError::Failed("the fake Airhouse is down".into()));
        }
        conn.lock()
            .unwrap()
            .execute_batch(&sql)
            .map_err(|e| SinkError::Failed(e.to_string()))?;
        self.appends.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn prepare(&self, _db: &DatabaseConnection, workspace_id: Uuid) -> Result<(), SinkError> {
        let conn = self
            .connection(workspace_id)
            .ok_or(SinkError::NotConfigured)?;
        conn.lock()
            .unwrap()
            .execute_batch(CREATE_TABLE)
            .map_err(|e| SinkError::Failed(e.to_string()))
    }

    async fn query(
        &self,
        _db: &DatabaseConnection,
        workspace_id: Uuid,
        sql: &str,
    ) -> Result<SqlRows, SinkError> {
        let conn = self
            .connection(workspace_id)
            .ok_or(SinkError::NotConfigured)?;
        self.queries.fetch_add(1, Ordering::SeqCst);
        let delay = self.query_delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        if self.fail_queries.load(Ordering::SeqCst) {
            return Err(SinkError::Failed("the fake Airhouse is down".into()));
        }
        // Every cell as text or NULL, as pgwire's text protocol delivers it.
        let wrapped = format!("SELECT COLUMNS(*)::VARCHAR FROM ({sql})");
        let conn = conn.lock().unwrap();
        let mut statement = conn
            .prepare(&wrapped)
            .map_err(|e| SinkError::Failed(format!("{e}: {sql}")))?;
        let mut rows = statement
            .query([])
            .map_err(|e| SinkError::Failed(format!("{e}: {sql}")))?;
        let mut out = SqlRows::default();
        while let Some(row) = rows
            .next()
            .map_err(|e| SinkError::Failed(format!("{e}: {sql}")))?
        {
            let width = row.as_ref().column_count();
            if out.columns.is_empty() {
                out.columns = (0..width)
                    .map(|i| {
                        row.as_ref()
                            .column_name(i)
                            .map(|n| n.to_string())
                            .unwrap_or_default()
                    })
                    .collect();
            }
            out.rows.push(
                (0..width)
                    .map(|i| row.get::<_, Option<String>>(i))
                    .collect::<Result<_, _>>()
                    .map_err(|e| SinkError::Failed(e.to_string()))?,
            );
        }
        Ok(out)
    }
}
