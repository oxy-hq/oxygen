//! Where the relay writes (OLTP change stream design §5.3): the workspace's
//! Airhouse, behind a small trait so the relay pass can be tested against an
//! in-memory DuckDB.
//!
//! [`AirhouseSink`] opens a **dedicated** connection for every call under
//! [`airhouse::SystemPurpose::OltpChanges`], never a pooled `tenant_client`:
//! [`ChangeSink::write`] runs `BEGIN ... COMMIT`, and a transaction must not
//! share a connection with anyone else.

use std::future::Future;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tokio_postgres::{Client, SimpleQueryMessage};
use uuid::Uuid;

use crate::app_events::mirror::workspace_has_airhouse;

/// Why a sink call failed: the store's own message, kept short.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct SinkError(pub String);

fn failed(e: impl std::fmt::Display) -> SinkError {
    SinkError(e.to_string().chars().take(500).collect())
}

/// A statement's failure, with the server's own message: a
/// `tokio_postgres::Error`'s `Display` is the bare "db error", and the relay
/// tells a missing schema or table from any other failure by that message.
/// A server error reads `[<SQLSTATE>] <message>`.
fn statement_failed(e: tokio_postgres::Error) -> SinkError {
    failed(oxy_oltp::connect::pg_detail(&e))
}

/// What every failure to reach Airhouse (the credential mint, the connect)
/// starts with, so it never reads as a statement's error: a connect can fail
/// with a server message that says `does not exist` (a role, a database, a
/// catalog to attach), and none of those means nothing was shipped.
const CONNECT: &str = "connect: ";

fn connect_failed(e: impl std::fmt::Display) -> SinkError {
    failed(format!("{CONNECT}{e}"))
}

/// The SQLSTATEs of a missing table and a missing schema, as
/// [`statement_failed`] leads with them.
const MISSING_STATES: [&str; 2] = ["[42P01] ", "[3F000] "];

/// DuckLake's catalog errors for a missing table or schema. Airhouse reports
/// them under `XX000`, with no SQLSTATE precise enough to match on
/// (`oxy_cameras::airhouse::is_missing_table`), so the message's own form is
/// matched, and only at the start of a statement's error.
const MISSING_CATALOG: [&str; 2] = [
    "[XX000] Catalog Error: Table with name ",
    "[XX000] Catalog Error: Schema with name ",
];

/// Whether `message`, a [`SinkError`]'s, is a statement's error saying its
/// table or schema does not exist. A connect error never is: it starts with
/// [`CONNECT`], and every match here is anchored at the start.
fn is_missing_statement(message: &str) -> bool {
    MISSING_STATES
        .iter()
        .any(|state| message.starts_with(state))
        || (MISSING_CATALOG.iter().any(|form| message.starts_with(form))
            && message.contains(" does not exist"))
}

/// What [`AirhouseSink::has_airhouse`] fails with in a process that has no
/// Airhouse token broker.
const NO_BROKER: &str = "this process has no Airhouse token broker (Airhouse is not configured)";

/// `Err` unless the process has an Airhouse token broker. Without one it
/// cannot tell whether any workspace has an Airhouse: that is the worker's
/// misconfiguration, never the workspace's, so the writer fails for the pass
/// instead of being held.
fn broker_present(present: bool) -> Result<(), SinkError> {
    if present {
        Ok(())
    } else {
        Err(failed(NO_BROKER))
    }
}

/// The relay's view of the workspace Airhouse.
#[async_trait]
pub trait ChangeSink: Send + Sync {
    /// Whether `workspace_id` has an active Airhouse tenant: `false` holds the
    /// writer. `Err` when that cannot be told, a process with no Airhouse
    /// token broker included, fails the writer for this pass.
    async fn has_airhouse(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<bool, SinkError>;
    /// One read statement; every value as text.
    async fn query(
        &self,
        workspace_id: Uuid,
        sql: &str,
    ) -> Result<Vec<Vec<Option<String>>>, SinkError>;
    /// DDL as Admin, each statement its own commit.
    async fn admin(&self, workspace_id: Uuid, statements: &[String]) -> Result<(), SinkError>;
    /// Rows as Writer, all statements in one transaction; rolled back on any failure.
    async fn write(&self, workspace_id: Uuid, statements: &[String]) -> Result<(), SinkError>;
    /// Whether `error`, from [`ChangeSink::query`], says the statement's table
    /// or schema does not exist: nothing was ever shipped there. Never true
    /// for a failure to reach the sink. The default suits a sink with no
    /// connect step, such as an in-memory DuckDB, whose catalog errors say
    /// `… does not exist!`.
    fn is_missing(&self, error: &SinkError) -> bool {
        error.0.contains("does not exist")
    }
}

static SINK: LazyLock<RwLock<Arc<dyn ChangeSink>>> =
    LazyLock::new(|| RwLock::new(Arc::new(AirhouseSink)));

/// Replace the process's sink. For tests, which have no Airhouse.
pub fn install_sink(sink: Arc<dyn ChangeSink>) {
    *SINK.write().unwrap_or_else(|e| e.into_inner()) = sink;
}

pub fn sink() -> Arc<dyn ChangeSink> {
    SINK.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// A sink every call of which fails once it has run for `limit`
/// ([`SINK_TIMEOUT`](super::SINK_TIMEOUT) in a pass), so a hung Airhouse
/// call cannot outlast the lease. A timed-out call is a failed one: the
/// writer fails for the pass. A `write` dropped after its `COMMIT` was sent
/// may still land, which is the relay that died after the commit: the next
/// pass ships the batch again, and the anti-join adds nothing twice.
pub struct Bounded {
    inner: Arc<dyn ChangeSink>,
    limit: Duration,
}

impl Bounded {
    pub fn new(inner: Arc<dyn ChangeSink>, limit: Duration) -> Self {
        Self { inner, limit }
    }

    async fn within<T>(
        &self,
        call: impl Future<Output = Result<T, SinkError>> + Send,
    ) -> Result<T, SinkError> {
        tokio::time::timeout(self.limit, call)
            .await
            .unwrap_or_else(|_| {
                let limit = self.limit.as_secs_f64();
                Err(failed(format!("timed out after {limit} s")))
            })
    }
}

#[async_trait]
impl ChangeSink for Bounded {
    async fn has_airhouse(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<bool, SinkError> {
        self.within(self.inner.has_airhouse(db, workspace_id)).await
    }

    async fn query(
        &self,
        workspace_id: Uuid,
        sql: &str,
    ) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        self.within(self.inner.query(workspace_id, sql)).await
    }

    async fn admin(&self, workspace_id: Uuid, statements: &[String]) -> Result<(), SinkError> {
        self.within(self.inner.admin(workspace_id, statements))
            .await
    }

    async fn write(&self, workspace_id: Uuid, statements: &[String]) -> Result<(), SinkError> {
        self.within(self.inner.write(workspace_id, statements))
            .await
    }

    fn is_missing(&self, error: &SinkError) -> bool {
        self.inner.is_missing(error)
    }
}

/// `BEGIN`, the statements, then `COMMIT`.
pub fn transaction(statements: &[String]) -> Vec<String> {
    let mut all = Vec::with_capacity(statements.len() + 2);
    all.push("BEGIN".to_owned());
    all.extend(statements.iter().cloned());
    all.push("COMMIT".to_owned());
    all
}

/// The production sink.
#[derive(Debug, Clone, Copy, Default)]
pub struct AirhouseSink;

impl AirhouseSink {
    async fn connect(workspace_id: Uuid, role: airhouse::UserRole) -> Result<Client, SinkError> {
        oxy_cameras::airhouse::connect(
            workspace_id,
            role,
            airhouse::SystemPurpose::OltpChanges,
            oxy_cameras::airhouse::ingest_ttl(),
        )
        .await
        .map_err(connect_failed)
    }
}

async fn run_all(client: &Client, statements: &[String]) -> Result<(), SinkError> {
    for sql in statements {
        client.simple_query(sql).await.map_err(statement_failed)?;
    }
    Ok(())
}

#[async_trait]
impl ChangeSink for AirhouseSink {
    async fn has_airhouse(
        &self,
        db: &DatabaseConnection,
        workspace_id: Uuid,
    ) -> Result<bool, SinkError> {
        // `workspace_has_airhouse` answers `false` with no broker too; only a
        // workspace without an active tenant may hold a writer.
        broker_present(airhouse::token_broker().is_some())?;
        workspace_has_airhouse(db, workspace_id)
            .await
            .map_err(failed)
    }

    async fn query(
        &self,
        workspace_id: Uuid,
        sql: &str,
    ) -> Result<Vec<Vec<Option<String>>>, SinkError> {
        let client = Self::connect(workspace_id, airhouse::UserRole::Writer).await?;
        let messages = client.simple_query(sql).await.map_err(statement_failed)?;
        Ok(messages
            .iter()
            .filter_map(|m| match m {
                SimpleQueryMessage::Row(row) => Some(
                    (0..row.len())
                        .map(|i| row.get(i).map(str::to_owned))
                        .collect(),
                ),
                _ => None,
            })
            .collect())
    }

    async fn admin(&self, workspace_id: Uuid, statements: &[String]) -> Result<(), SinkError> {
        if statements.is_empty() {
            return Ok(());
        }
        let client = Self::connect(workspace_id, airhouse::UserRole::Admin).await?;
        run_all(&client, statements).await
    }

    async fn write(&self, workspace_id: Uuid, statements: &[String]) -> Result<(), SinkError> {
        if statements.is_empty() {
            return Ok(());
        }
        let client = Self::connect(workspace_id, airhouse::UserRole::Writer).await?;
        let result = run_all(&client, &transaction(statements)).await;
        if result.is_err() {
            // The original error is the one worth reporting.
            let _ = client.simple_query("ROLLBACK").await;
        }
        result
    }

    fn is_missing(&self, error: &SinkError) -> bool {
        is_missing_statement(&error.0)
    }
}

#[cfg(test)]
mod tests;
