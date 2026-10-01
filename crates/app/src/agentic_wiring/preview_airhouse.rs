//! The Airhouse connector a workspace preview run sends everything through.
//!
//! A preview reads live tables and writes only its own schemas,
//! `preview_<key>__<live schema>`. Three fences keep its writes there: the
//! step's rewrite (`airhouse::preview_sql::rewrite`, run by the host), and the
//! two here, which see every statement whatever sent it — a rewritten step, an
//! agent's SQL, a semantic query:
//!
//! 1. **The verifier.** Every statement is checked by
//!    `preview_sql::verify_statements` before anything is sent; one statement
//!    writing outside the namespace refuses the whole batch. What is sent is
//!    the statement as verified, re-rendered from the checked tree.
//! 2. **The credential.** A batch that writes runs on a Writer scoped to
//!    exactly the preview schemas it writes (`AirhouseTokenBroker::
//!    mint_for_preview`), and Airhouse refuses any write outside that scope.
//!    An Airhouse older than 0.1.49 cannot confine one: the broker asks
//!    (`GET /admin/v1/capabilities`) before it mints a preview Writer, and
//!    refuses a Writer whose `write_schemas` echo is not exactly its scope, so
//!    on such an Airhouse preview writes stay held. A batch that only reads
//!    runs on a Reader.
//!
//! Reads get the overlay (D6): a read of `S.t` goes to the preview's copy when
//! the run's [`ShadowMap`] has one, unless the run reads live only. The reads
//! inside a write (`INSERT … SELECT`) do not: a procedure step's SQL was
//! overlaid by `preview_sql::rewrite` before it got here.
//!
//! **A batch is one connection.** Its statements are sent in order on one
//! pooled connection. A batch with `BEGIN … COMMIT` holds its connection
//! alone, from identities only transactions use, so its transaction shares a
//! session with nothing else; a statement that fails inside it stops the
//! batch and rolls it back, and so does a caller that gives up mid-way. A
//! connection whose rollback fails, or does not answer in bounded time, is
//! poisoned (dropped from the pool), never handed out again. Every other batch
//! — reads, streams, autocommitted writes — shares its connection as the rest
//! of the Airhouse pool does, so a task can read while it drains a stream.
//!
//! Schema DDL (`CREATE`/`DROP SCHEMA`, dropping a relation for the TTL) is
//! refused here, even for the preview's own schemas: a schema is created only
//! after its registry row (`previews::registry::ensure_schema`) and dropped
//! only by the TTL sweep, and [`system_ddl`] builds those fixed statements
//! from names the namespace owns and sends them on a system Writer. (Airhouse
//! would let the scoped Writer create one of its schemas, never drop one.)
//!
//! **Every trait method is stated** (`tests::every_trait_method_is_stated`):
//! a defaulted method on a wrapper answers without the wrapper deciding.

// `ConnectorError` is the `DatabaseConnector` trait's error; the helpers here
// return it unchanged rather than boxing it only to unbox it at the trait.
#![allow(clippy::result_large_err)]

mod backend;
mod batch;
mod ports;

#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock, PoisonError, RwLock};

use agentic_connector::{
    AsArrowConnector, ConnectorError, DatabaseConnector, ExecutionResult, SchemaInfo, SqlDialect,
    SqlTransaction,
};
use agentic_core::result::TypedRowStream;
use airhouse::preview_sql::{PreviewNamespace, Refused, RewriteOptions, ShadowMap};
use async_trait::async_trait;
use uuid::Uuid;

pub use crate::server::previews::ddl::Ddl;
pub use crate::server::previews::ddl_airhouse::system_ddl;
pub use backend::{AirhouseBackend, Hold, Lease, PreviewAirhouseBackend, Scope, Use};
pub use ports::{PipelineWriter, PreviewAirhousePorts, WorkspaceAirhouse, Writers};

use batch::{Call, Plan};

/// The connector for preview `ns` of `workspace_id`, on the workspace's
/// Airhouse. `shadow` is the run's map, shared with the host that updates it
/// after each step; `opts` carries the workspace's DuckLake catalog (a name
/// qualified with any other catalog is refused; `None` refuses every one) and
/// the run's `read_live_only`.
pub fn connector(
    workspace_id: Uuid,
    ns: PreviewNamespace,
    shadow: Arc<RwLock<ShadowMap>>,
    opts: RewriteOptions,
) -> Arc<dyn DatabaseConnector> {
    connector_on(&WorkspaceAirhouse, workspace_id, ns, shadow, opts)
}

/// [`connector`] on any Airhouse `ports` names (the workspace's, or an
/// in-process stand-in).
pub fn connector_on(
    ports: &dyn PreviewAirhousePorts,
    workspace_id: Uuid,
    ns: PreviewNamespace,
    shadow: Arc<RwLock<ShadowMap>>,
    opts: RewriteOptions,
) -> Arc<dyn DatabaseConnector> {
    let backend = ports.backend(workspace_id, &ns);
    Arc::new(PreviewAirhouseConnector::new(ns, shadow, opts, backend))
}

pub struct PreviewAirhouseConnector {
    ns: PreviewNamespace,
    shadow: Arc<RwLock<ShadowMap>>,
    opts: RewriteOptions,
    backend: Arc<dyn PreviewAirhouseBackend>,
    /// The Reader's schema, fetched by `prepare_schema`.
    schema: OnceLock<SchemaInfo>,
}

impl PreviewAirhouseConnector {
    /// Over any backend: [`connector`] passes the workspace's Airhouse, a test
    /// or an in-process stand-in passes its own.
    pub fn new(
        ns: PreviewNamespace,
        shadow: Arc<RwLock<ShadowMap>>,
        opts: RewriteOptions,
        backend: Arc<dyn PreviewAirhouseBackend>,
    ) -> Self {
        Self {
            ns,
            shadow,
            opts,
            backend,
            schema: OnceLock::new(),
        }
    }

    /// Verify and overlay `sql` against the run's current shadow map. A
    /// refusal sends nothing.
    fn plan(&self, sql: &str) -> Result<Plan, ConnectorError> {
        let shadow = self.shadow.read().unwrap_or_else(PoisonError::into_inner);
        batch::plan(sql, &self.ns, &shadow, &self.opts).map_err(|r| refused(&self.ns, &r))
    }

    async fn run(
        &self,
        plan: Plan,
        call: Call<'_>,
    ) -> Result<Option<ExecutionResult>, ConnectorError> {
        let lease = self.backend.checkout(&plan.scope, plan.usage).await?;
        batch::run(lease, &plan.statements, call).await
    }

    async fn stream(&self, plan: Plan, untyped: bool) -> Result<TypedRowStream, ConnectorError> {
        if let Some(why) = batch::unstreamable(&plan.statements) {
            return Err(ConnectorError::Other(format!(
                "refused in workspace preview {}: {why}. Nothing was sent.",
                self.ns.key()
            )));
        }
        let lease = self.backend.checkout(&plan.scope, plan.usage).await?;
        batch::stream(lease, &plan.statements, untyped).await
    }
}

fn refused(ns: &PreviewNamespace, why: &Refused) -> ConnectorError {
    ConnectorError::Other(format!(
        "refused in workspace preview {}: {why}. Nothing was sent.",
        ns.key()
    ))
}

#[async_trait]
impl DatabaseConnector for PreviewAirhouseConnector {
    fn dialect(&self) -> SqlDialect {
        SqlDialect::DuckDb
    }

    async fn execute_query(
        &self,
        sql: &str,
        sample_limit: u64,
    ) -> Result<ExecutionResult, ConnectorError> {
        let plan = self.plan(sql)?;
        let result = self.run(plan, Call::Query(sample_limit)).await?;
        Ok(result.unwrap_or_else(ExecutionResult::empty))
    }

    async fn execute_query_full(&self, sql: &str) -> Result<TypedRowStream, ConnectorError> {
        let plan = self.plan(sql)?;
        self.stream(plan, false).await
    }

    async fn execute_query_full_untyped(
        &self,
        sql: &str,
    ) -> Result<TypedRowStream, ConnectorError> {
        let plan = self.plan(sql)?;
        self.stream(plan, true).await
    }

    /// A pinned session this connector could not verify statement by
    /// statement. A batch may carry its own `BEGIN … COMMIT` instead.
    async fn begin_transaction(&self) -> Result<Box<dyn SqlTransaction>, ConnectorError> {
        Err(ConnectorError::Other(format!(
            "refused in workspace preview {}: send BEGIN … COMMIT inside one batch instead of \
             opening a transaction. Nothing was sent.",
            self.ns.key()
        )))
    }

    /// The Arrow path would run SQL on the inner connector unverified.
    fn as_arrow(&self) -> Option<&dyn AsArrowConnector> {
        None
    }

    async fn execute_statement(&self, sql: &str) -> Result<(), ConnectorError> {
        let plan = self.plan(sql)?;
        self.run(plan, Call::Statement).await.map(|_| ())
    }

    async fn execute_statement_tagged(&self, sql: &str, tag: &str) -> Result<(), ConnectorError> {
        let plan = self.plan(sql)?;
        self.run(plan, Call::Tagged(tag)).await.map(|_| ())
    }

    /// Fetch the schema once, on a Reader.
    async fn prepare_schema(&self) -> Result<(), ConnectorError> {
        if self.schema.get().is_none() {
            let lease = self.backend.checkout(&Scope::Reader, Use::Shared).await?;
            let _ = self.schema.set(lease.connector().introspect_schema()?);
        }
        Ok(())
    }

    fn introspect_schema(&self) -> Result<SchemaInfo, ConnectorError> {
        Ok(self.schema.get().cloned().unwrap_or_default())
    }
}
