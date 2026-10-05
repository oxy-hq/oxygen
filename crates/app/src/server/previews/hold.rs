//! The connector backstop: a warehouse connector that cannot write.
//!
//! A procedure's `execute_sql` steps are held visibly at the step
//! (`WorkspaceContext::review_sql`). Every other path to a warehouse inside a
//! preview run — an agent's SQL, a semantic query, anything a later change
//! adds — reaches it through the connector the preview platform hands out, and
//! [`HoldingConnector`] is that connector: it forwards a statement only when
//! [`super::sql_kind`] classifies all of it as a read, and refuses everything
//! else without sending a byte.
//!
//! **On Postgres the session is read-only too** ([`pg`]): every statement it
//! forwards is sent right after [`pg::READ_ONLY_SESSION`], under one lock per
//! connector held from the `SET` until the forwarded call returns, so no
//! other call through this connector can run between the two. A read the
//! classifier passes therefore cannot write through a function it calls,
//! even after an earlier call switched the session back; if the `SET` fails,
//! nothing is forwarded. Its reads are served without the temp table the
//! Postgres connector's sampler creates, which a read-only session refuses.
//! Redshift ([`Session::ClassifierOnly`], chosen from the database's
//! configured type — it reports the Postgres dialect) gets no `SET`: there the
//! classifier is the whole guard, as it was before.
//!
//! Phase 2a holds **every** write, Airhouse included, so the preview platform
//! wraps every connector it hands out, not only customer warehouses.
//!
//! **Every method is overridden.** The failure mode of a wrapper is a
//! defaulted trait method silently answering — the trap `agentic-airway`'s
//! `BoxedSourceConnector` documents — and here a default would not answer
//! wrongly, it would *forward*: `execute_statement_tagged`'s default calls
//! `execute_statement` on `self`, but a future default could call the inner
//! connector directly. `every_trait_method_is_overridden` (a source scan of
//! `DatabaseConnector`) fails the build when the trait grows a method this
//! impl does not state.

use std::sync::Arc;

use agentic_connector::{
    AsArrowConnector, ConnectorError, DatabaseConnector, ExecutionResult, QueryFailedDetails,
    SchemaInfo, SqlDialect, SqlTransaction, StringLiteral,
};
use agentic_core::result::TypedRowStream;
use async_trait::async_trait;
use oxy::config::model::DatabaseType;
use tokio::sync::{Mutex, MutexGuard};

use super::request_hold::{HeldSink, HeldStatement, HoldScope};
use super::sql_kind::{StatementKind, classify, first_non_read, is_all_read};

/// Whether a Postgres-dialect connector's session is made read-only.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Session {
    /// `SET … READ ONLY` before every forwarded statement (Postgres).
    #[default]
    ReadOnly,
    /// No `SET`: the classifier is the whole guard. Redshift, which speaks the
    /// Postgres dialect through the same connector but is not known to accept
    /// the `SET` — refusing it would hold every read.
    ClassifierOnly,
}

impl Session {
    /// The session for a database of `database_type`.
    pub fn for_type(database_type: &DatabaseType) -> Self {
        match database_type {
            DatabaseType::Redshift(_) => Self::ClassifierOnly,
            _ => Self::ReadOnly,
        }
    }

    /// The session for `database` in `config`; [`Session::ReadOnly`] when it
    /// does not resolve (a non-Postgres connector ignores it).
    pub fn of<S>(config: &oxy::config::ConfigManager<S>, database: &str) -> Self {
        config
            .resolve_database(database)
            .map(|db| Self::for_type(&db.database_type))
            .unwrap_or_default()
    }
}

/// Held by an admitted call from its session `SET` until its forwarded call
/// returns; `None` where there is no session to protect.
type Turn<'a> = Option<MutexGuard<'a, ()>>;

/// Wraps a connector so that only reads reach it.
pub struct HoldingConnector {
    inner: Arc<dyn DatabaseConnector>,
    database: String,
    /// Where the write was held, as the refusal says it
    /// ([`HoldScope::place`]).
    place: &'static str,
    /// Told about each refused statement before the refusal returns.
    sink: Option<Arc<dyn HeldSink>>,
    session: Session,
    /// Serialises the session `SET` with the statement it guards.
    turn: Mutex<()>,
}

impl HoldingConnector {
    /// `database` is the `config.yml` name, used in the refusal so the person
    /// reading a failed step or agent turn knows which warehouse was protected.
    pub fn new(inner: Arc<dyn DatabaseConnector>, database: impl Into<String>) -> Self {
        Self::under(inner, database, &HoldScope::default())
    }

    /// Held under `hold`: its place names the refusal, its sink hears it.
    pub fn under(
        inner: Arc<dyn DatabaseConnector>,
        database: impl Into<String>,
        hold: &HoldScope,
    ) -> Self {
        Self {
            inner,
            database: database.into(),
            place: hold.place(),
            sink: hold.sink(),
            session: Session::ReadOnly,
            turn: Mutex::new(()),
        }
    }

    /// With `session` (what the database's configured type calls for).
    pub fn with_session(mut self, session: Session) -> Self {
        self.session = session;
        self
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    /// Postgres whose session is made read-only: reads skip the temp-table
    /// sampler and every forwarded statement follows the `SET`.
    fn is_postgres(&self) -> bool {
        self.inner.dialect() == SqlDialect::Postgres && self.session == Session::ReadOnly
    }

    /// Whether `sql` is the read-only `SET` itself, which an outer holding
    /// connector sends this one when two are stacked: it only narrows the
    /// session, so it is admitted, and this connector's own session `SET`
    /// is what sends it.
    fn is_session_set(&self, sql: &str) -> bool {
        self.is_postgres() && sql == pg::READ_ONLY_SESSION
    }

    /// `Ok` only when every statement in `sql` is a read and, on Postgres, the
    /// session has just been made read-only. The caller keeps the returned
    /// turn until its forwarded call returns.
    async fn admit(&self, sql: &str) -> Result<Turn<'_>, ConnectorError> {
        if self.is_session_set(sql) {
            return self.read_only_session(sql).await.map(Some);
        }
        let kinds = classify(self.inner.dialect(), sql);
        if !is_all_read(&kinds) {
            if let Some(kind) = first_non_read(&kinds) {
                self.note(kind).await;
            }
            return Err(held(sql, held_message(self.place, &self.database, &kinds)));
        }
        if self.is_postgres() {
            return self.read_only_session(sql).await.map(Some);
        }
        Ok(None)
    }

    /// Make the inner Postgres session read-only — again before every
    /// statement, not once: a function the classifier passed can switch
    /// `default_transaction_read_only` off from inside its body, which would
    /// leave the *next* statement writable (verified against Postgres; the
    /// statement that flips it is already read-only).
    ///
    /// Only the server **rejecting** the `SET` (a `QueryFailed`) refuses `sql`
    /// as held and noted — once: a refusal from a stacked holding connector
    /// below was noted there. Any other failure — the warehouse unreachable,
    /// auth or TLS refused (`ConnectionError`), a driver error (`Other`) —
    /// passes through unchanged and unnoted: nothing was held, the call simply
    /// could not run, and calling it `preview_read_only` would send the
    /// operator after a write that never existed. Nothing is forwarded either
    /// way.
    async fn read_only_session(&self, sql: &str) -> Result<MutexGuard<'_, ()>, ConnectorError> {
        let turn = self.turn.lock().await;
        let e = match self.inner.execute_statement(pg::READ_ONLY_SESSION).await {
            Ok(()) => return Ok(turn),
            Err(e) if is_held(&e) => return Err(e),
            Err(e @ ConnectorError::QueryFailed(_)) => e,
            Err(e) => return Err(e),
        };
        self.note(&StatementKind::Write {
            verb: "READ_ONLY_SESSION".to_string(),
            targets: Vec::new(),
        })
        .await;
        Err(held(
            sql,
            format!(
                "held: `{}` could not be made read-only in {} ({e}), so nothing is sent to it. \
                 Nothing was sent.",
                self.database, self.place
            ),
        ))
    }

    async fn refuse(&self, what: &str) -> ConnectorError {
        self.note(&StatementKind::Write {
            verb: "BEGIN".to_string(),
            targets: Vec::new(),
        })
        .await;
        held(
            "",
            format!(
                "held: `{}` cannot be written in {}, so {what} is refused. Nothing was sent.",
                self.database, self.place
            ),
        )
    }

    async fn note(&self, kind: &StatementKind) {
        if let Some(sink) = &self.sink {
            sink.held(HeldStatement {
                database: &self.database,
                dialect: self.inner.dialect(),
                kind,
            })
            .await;
        }
    }
}

/// The `code` a held statement's error carries — the same `preview_read_only`
/// a refused route answers — so an HTTP surface can tell a preview refusal
/// from a query that failed (`data::agentic_error_response` answers `409`).
pub const HELD_CODE: &str = "preview_read_only";

/// Whether `e` is a holding connector's refusal.
fn is_held(e: &ConnectorError) -> bool {
    matches!(e, ConnectorError::QueryFailed(d) if d.code.as_deref() == Some(HELD_CODE))
}

/// A refusal: a failed query, typed with [`HELD_CODE`], so a caller branching
/// on it never has to read the message.
fn held(sql: &str, message: String) -> ConnectorError {
    ConnectorError::QueryFailed(QueryFailedDetails {
        sql: sql.to_string(),
        message,
        code: Some(HELD_CODE.to_string()),
        ..Default::default()
    })
}

/// What the refusal says: the verb and targets of the first statement that is
/// not a read, so an agent (or a person) can see exactly what was held.
fn held_message(place: &str, database: &str, kinds: &[StatementKind]) -> String {
    let what = match first_non_read(kinds) {
        Some(StatementKind::Write { verb, targets }) if targets.is_empty() => format!("`{verb}`"),
        Some(StatementKind::Write { verb, targets }) => {
            format!("`{verb}` on {}", targets.join(", "))
        }
        Some(StatementKind::Unclassified(reason)) => {
            format!("a statement that could not be classified as a read ({reason})")
        }
        Some(StatementKind::Read) | None => "a statement".to_string(),
    };
    format!(
        "held: `{database}` cannot be written in {place}, and this SQL runs {what}. Only reads \
         are sent. Nothing was sent."
    )
}

#[async_trait]
impl DatabaseConnector for HoldingConnector {
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
        let _turn = self.admit(sql).await?;
        if self.is_postgres() {
            // The connector's own sampler writes a temp table, which a
            // read-only session refuses.
            return pg::sample(&*self.inner, sql, sample_limit).await;
        }
        self.inner.execute_query(sql, sample_limit).await
    }

    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        let _turn = self.admit(sql).await?;
        self.inner.execute_query_full(sql).await
    }

    async fn execute_query_full_untyped(
        &self,
        sql: &str,
    ) -> Result<TypedRowStream, ConnectorError> {
        let _turn = self.admit(sql).await?;
        self.inner.execute_query_full_untyped(sql).await
    }

    /// A transaction is a session the wrapper could not see into statement by
    /// statement, and its only purpose is writing. Refused outright.
    async fn begin_transaction(&self) -> Result<Box<dyn SqlTransaction>, ConnectorError> {
        Err(self.refuse("a transaction").await)
    }

    /// The Arrow path executes SQL on the inner connector directly, so handing
    /// it out would bypass [`Self::admit`]. Callers fall back to
    /// `execute_query_full`, which is guarded.
    fn as_arrow(&self) -> Option<&dyn AsArrowConnector> {
        None
    }

    /// On Postgres a read goes through `execute_query_full`: the connector's
    /// `execute_statement` is its temp-table `execute_query`.
    async fn execute_statement(&self, sql: &str) -> Result<(), ConnectorError> {
        let _turn = self.admit(sql).await?;
        if self.is_session_set(sql) {
            // `admit` sent it.
            return Ok(());
        }
        if self.is_postgres() {
            return self.inner.execute_query_full(sql).await.map(|_| ());
        }
        self.inner.execute_statement(sql).await
    }

    async fn execute_statement_tagged(&self, sql: &str, tag: &str) -> Result<(), ConnectorError> {
        let _turn = self.admit(sql).await?;
        if self.is_postgres() {
            let tagged = agentic_connector::with_trailing_comment(
                agentic_connector::normalize_sql(sql),
                tag,
            );
            return self.inner.execute_query_full(&tagged).await.map(|_| ());
        }
        self.inner.execute_statement_tagged(sql, tag).await
    }

    /// A no-op: some connectors prepare by opening sessions or creating
    /// scratch objects, and a preview has no reason to let them.
    async fn prepare_schema(&self) -> Result<(), ConnectorError> {
        Ok(())
    }

    fn introspect_schema(&self) -> Result<SchemaInfo, ConnectorError> {
        self.inner.introspect_schema()
    }
}

#[path = "hold_pg.rs"]
pub mod pg;

#[cfg(test)]
#[path = "hold_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "hold_session_tests.rs"]
mod session_tests;
