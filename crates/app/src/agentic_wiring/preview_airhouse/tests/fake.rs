//! A fake Airhouse behind the S5 port: a fresh connection per checkout, one
//! exclusive slot for transactions (as the pool's transaction identities
//! are), and a record, per connection, of every statement that reaches one.

use std::future::pending;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agentic_connector::{ConnectorError, DatabaseConnector, ExecutionResult, SqlDialect};
use agentic_core::result::TypedRowStream;
use async_trait::async_trait;
use tokio::sync::OwnedMutexGuard;

use super::super::{Hold, Lease, PreviewAirhouseBackend, Scope, Use};

/// What reached the fake Airhouse.
#[derive(Default)]
pub struct Log {
    /// Every checkout, in order; connection `#n` is the n-th.
    pub checkouts: Vec<(Scope, Use)>,
    /// `#<connection> <method> <sql>`, in the order they arrived.
    pub sent: Vec<String>,
}

#[derive(Default)]
pub struct FakeAirhouse {
    pub log: Arc<Mutex<Log>>,
    /// A statement containing any of these fails.
    pub fail_on: Vec<&'static str>,
    /// A statement containing this never finishes.
    pub hang_on: Option<&'static str>,
    /// Set when the last transaction lease handed out is dropped.
    pub released: Arc<AtomicBool>,
    /// Set when a transaction lease is poisoned.
    pub poisoned: Arc<AtomicBool>,
    /// The one connection transactions get, held for the lease's life.
    pub tx_slot: Arc<tokio::sync::Mutex<()>>,
}

impl FakeAirhouse {
    pub fn failing_on(needles: &[&'static str]) -> Self {
        Self {
            fail_on: needles.to_vec(),
            ..Self::default()
        }
    }

    pub fn hanging_on(needle: &'static str) -> Self {
        Self {
            hang_on: Some(needle),
            ..Self::default()
        }
    }

    pub fn checkouts(&self) -> Vec<Scope> {
        let log = self.log.lock().unwrap();
        log.checkouts.iter().map(|c| c.0.clone()).collect()
    }

    pub fn usages(&self) -> Vec<Use> {
        let log = self.log.lock().unwrap();
        log.checkouts.iter().map(|c| c.1).collect()
    }

    pub fn sent(&self) -> Vec<String> {
        self.log.lock().unwrap().sent.clone()
    }

    pub fn is_released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::SeqCst)
    }
}

/// A transaction lease's hold on the slot: records poison, and release on drop.
struct FakeHold {
    _slot: OwnedMutexGuard<()>,
    released: Arc<AtomicBool>,
    poisoned: Arc<AtomicBool>,
}

impl Hold for FakeHold {
    fn poison(&mut self) {
        self.poisoned.store(true, Ordering::SeqCst);
    }
}

impl Drop for FakeHold {
    fn drop(&mut self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

#[async_trait]
impl PreviewAirhouseBackend for FakeAirhouse {
    async fn checkout(&self, scope: &Scope, usage: Use) -> Result<Lease, ConnectorError> {
        let n = {
            let mut log = self.log.lock().unwrap();
            log.checkouts.push((scope.clone(), usage));
            log.checkouts.len()
        };
        let conn = Arc::new(FakeConn {
            n,
            log: Arc::clone(&self.log),
            fail_on: self.fail_on.clone(),
            hang_on: self.hang_on,
        });
        if usage == Use::Shared {
            return Ok(Lease::new(conn));
        }
        let slot = Arc::clone(&self.tx_slot).lock_owned().await;
        self.released.store(false, Ordering::SeqCst);
        let hold = FakeHold {
            _slot: slot,
            released: Arc::clone(&self.released),
            poisoned: Arc::clone(&self.poisoned),
        };
        Ok(Lease::held(conn, hold))
    }
}

struct FakeConn {
    n: usize,
    log: Arc<Mutex<Log>>,
    fail_on: Vec<&'static str>,
    hang_on: Option<&'static str>,
}

impl FakeConn {
    async fn receive(&self, method: &str, sql: &str) -> Result<(), ConnectorError> {
        let line = format!("#{} {method} {sql}", self.n);
        self.log.lock().unwrap().sent.push(line);
        if self.hang_on.is_some_and(|needle| sql.contains(needle)) {
            pending::<()>().await;
        }
        if self.fail_on.iter().any(|needle| sql.contains(needle)) {
            return Err(ConnectorError::query_failed(
                sql,
                "the fake Airhouse failed it",
            ));
        }
        Ok(())
    }
}

fn empty_stream() -> TypedRowStream {
    TypedRowStream {
        columns: vec![],
        rows: Box::pin(futures::stream::empty()),
        truncated: None,
    }
}

#[async_trait]
impl DatabaseConnector for FakeConn {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::DuckDb
    }
    async fn execute_query(&self, sql: &str, _: u64) -> Result<ExecutionResult, ConnectorError> {
        self.receive("query", sql).await?;
        Ok(ExecutionResult::empty())
    }
    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        self.receive("full", sql).await?;
        Ok(empty_stream())
    }
    async fn execute_query_full_untyped(
        &self,
        sql: &str,
    ) -> Result<TypedRowStream, ConnectorError> {
        self.receive("untyped", sql).await?;
        Ok(empty_stream())
    }
    async fn execute_statement(&self, sql: &str) -> Result<(), ConnectorError> {
        self.receive("statement", sql).await
    }
    async fn execute_statement_tagged(&self, sql: &str, _: &str) -> Result<(), ConnectorError> {
        self.receive("tagged", sql).await
    }
}
