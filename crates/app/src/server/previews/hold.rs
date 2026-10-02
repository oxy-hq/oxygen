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

use super::sql_kind::{StatementKind, classify, first_non_read, is_all_read};

/// Wraps a connector so that only reads reach it.
pub struct HoldingConnector {
    inner: Arc<dyn DatabaseConnector>,
    database: String,
}

impl HoldingConnector {
    /// `database` is the `config.yml` name, used in the refusal so the person
    /// reading a failed step or agent turn knows which warehouse was protected.
    pub fn new(inner: Arc<dyn DatabaseConnector>, database: impl Into<String>) -> Self {
        Self {
            inner,
            database: database.into(),
        }
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    /// `Ok` only when every statement in `sql` is a read.
    fn admit(&self, sql: &str) -> Result<(), ConnectorError> {
        let kinds = classify(self.inner.dialect(), sql);
        if is_all_read(&kinds) {
            return Ok(());
        }
        Err(held(sql, held_message(&self.database, &kinds)))
    }

    fn refuse(&self, what: &str) -> ConnectorError {
        held(
            "",
            format!(
                "held: `{}` cannot be written in a workspace preview, so {what} is refused. \
                 Nothing was sent.",
                self.database
            ),
        )
    }
}

/// The `code` a held statement's error carries — the same `preview_read_only`
/// a refused route answers — so an HTTP surface can tell a preview refusal
/// from a query that failed (`data::agentic_error_response` answers `409`).
pub const HELD_CODE: &str = "preview_read_only";

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
fn held_message(database: &str, kinds: &[StatementKind]) -> String {
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
        "held: `{database}` cannot be written in a workspace preview, and this SQL runs {what}. \
         Only reads are sent. Nothing was sent."
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
        self.admit(sql)?;
        self.inner.execute_query(sql, sample_limit).await
    }

    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        self.admit(sql)?;
        self.inner.execute_query_full(sql).await
    }

    async fn execute_query_full_untyped(
        &self,
        sql: &str,
    ) -> Result<TypedRowStream, ConnectorError> {
        self.admit(sql)?;
        self.inner.execute_query_full_untyped(sql).await
    }

    /// A transaction is a session the wrapper could not see into statement by
    /// statement, and its only purpose is writing. Refused outright.
    async fn begin_transaction(&self) -> Result<Box<dyn SqlTransaction>, ConnectorError> {
        Err(self.refuse("a transaction"))
    }

    /// The Arrow path executes SQL on the inner connector directly, so handing
    /// it out would bypass [`Self::admit`]. Callers fall back to
    /// `execute_query_full`, which is guarded.
    fn as_arrow(&self) -> Option<&dyn AsArrowConnector> {
        None
    }

    async fn execute_statement(&self, sql: &str) -> Result<(), ConnectorError> {
        self.admit(sql)?;
        self.inner.execute_statement(sql).await
    }

    async fn execute_statement_tagged(&self, sql: &str, tag: &str) -> Result<(), ConnectorError> {
        self.admit(sql)?;
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

#[cfg(test)]
#[path = "hold_tests.rs"]
mod tests;
