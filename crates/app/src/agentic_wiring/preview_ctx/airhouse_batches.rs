//! Knowing when a reviewed step's batch is done.
//!
//! A step's review plans its copies and records them before the step is sent
//! (`airhouse_writes`). Until that batch has run, its copies may not exist
//! yet, and a later review must not mistake one for a copy a crash left
//! unmade and plan it again ([`Planned`]). Once the batch is done — sent,
//! failed, or abandoned mid-way — a copy that is still missing really is
//! missing (the batch failed before it ran), and the next review of a write
//! to that table plans it again.
//!
//! [`Batches`] is the run's Airhouse connector with that bookkeeping: every
//! call that takes SQL settles the reviewed batch it carries when the call
//! ends, however it ends (a guard, so a cancelled call settles too).

use std::sync::{Arc, Mutex, PoisonError};

use agentic_connector::{
    AsArrowConnector, ConnectorError, DatabaseConnector, ExecutionResult, SchemaInfo, SqlDialect,
    SqlTransaction, StringLiteral,
};
use agentic_core::result::TypedRowStream;
use async_trait::async_trait;

type Live = (String, String);

/// Tables whose copy a reviewed, not yet finished batch carries, by batch.
#[derive(Default)]
pub(super) struct Planned(Mutex<Vec<(String, Vec<Live>)>>);

impl Planned {
    /// Batch `sql` carries copies of (or writes to) `tables`.
    pub(super) fn add(&self, sql: String, tables: Vec<Live>) {
        self.lock().push((sql, tables));
    }

    /// Whether an unfinished batch carries `live`.
    pub(super) fn contains(&self, live: &Live) -> bool {
        self.lock().iter().any(|(_, tables)| tables.contains(live))
    }

    /// Batch `sql` is done: one entry for it goes.
    fn settle(&self, sql: &str) {
        let mut entries = self.lock();
        if let Some(at) = entries.iter().position(|(batch, _)| batch == sql) {
            entries.remove(at);
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<(String, Vec<Live>)>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Settles `sql` when dropped: after the call, or when it is abandoned.
struct Settle<'a> {
    planned: &'a Planned,
    sql: &'a str,
}

impl Drop for Settle<'_> {
    fn drop(&mut self) {
        self.planned.settle(self.sql);
    }
}

/// The run's Airhouse connector ([`super::PreviewAirhouse::connector`]),
/// settling reviewed batches as they end. Every method is stated and passes
/// through to the preview Airhouse connector, which does all the fencing.
pub(super) struct Batches {
    pub(super) inner: Arc<dyn DatabaseConnector>,
    pub(super) planned: Arc<Planned>,
}

impl Batches {
    fn settling<'a>(&'a self, sql: &'a str) -> Settle<'a> {
        Settle {
            planned: &self.planned,
            sql,
        }
    }
}

#[async_trait]
impl DatabaseConnector for Batches {
    fn dialect(&self) -> SqlDialect {
        self.inner.dialect()
    }

    fn string_literal(&self) -> StringLiteral {
        self.inner.string_literal()
    }

    async fn execute_query(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        let _settle = self.settling(sql);
        self.inner.execute_query(sql, sample_limit).await
    }

    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        let _settle = self.settling(sql);
        self.inner.execute_query_full(sql).await
    }

    async fn execute_query_full_untyped(
        &self,
        sql: &str,
    ) -> Result<TypedRowStream, ConnectorError> {
        let _settle = self.settling(sql);
        self.inner.execute_query_full_untyped(sql).await
    }

    async fn begin_transaction(&self) -> Result<Box<dyn SqlTransaction>, ConnectorError> {
        self.inner.begin_transaction().await
    }

    /// `None`: the Arrow path would run SQL outside the preview connector.
    fn as_arrow(&self) -> Option<&dyn AsArrowConnector> {
        None
    }

    async fn execute_statement(&self, sql: &str) -> Result<(), ConnectorError> {
        let _settle = self.settling(sql);
        self.inner.execute_statement(sql).await
    }

    async fn execute_statement_tagged(&self, sql: &str, tag: &str) -> Result<(), ConnectorError> {
        let _settle = self.settling(sql);
        self.inner.execute_statement_tagged(sql, tag).await
    }

    async fn prepare_schema(&self) -> Result<(), ConnectorError> {
        self.inner.prepare_schema().await
    }

    fn introspect_schema(&self) -> Result<SchemaInfo, ConnectorError> {
        self.inner.introspect_schema()
    }
}
