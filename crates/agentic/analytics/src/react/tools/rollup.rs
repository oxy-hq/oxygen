//! `query`'s rollup tier: a `semantic` query that a live pre-aggregation
//! covers is answered from the rollup's Parquet instead of the warehouse.
//!
//! The rule is the FSM's (`AnalyticsSolver::build_semantic_payload`):
//! [`try_resolve_preagg`] with the catalog's live rollups, so a manifest row
//! built from a since-edited definition is declined rather than served. Raw
//! `sql` and `verified` queries never reach here; nothing the model wrote is
//! matched against a rollup.
//!
//! A rollup read is not a warehouse query. It runs Oxy's own generated SQL
//! (`read_parquet` over a file the rebuild wrote) in an in-memory DuckDB, so
//! the customer's warehouse is not touched. That SQL still passes the
//! read-only gate before it runs; a refusal declines the rollup.

use std::sync::Arc;

use agentic_connector::{ExecutionResult, SqlDialect};
use agentic_semantic::compile::{CompiledQuery, PreaggContext, PreaggSource, try_resolve_preagg};
use async_trait::async_trait;
use oxy_airlayer_compat::engine::query::QueryRequest;

use crate::SemanticCatalog;
use crate::react::read_only::{gate, sqlparser_dialect};

/// A rollup that covers one query: the DuckDB SQL and the Parquet it reads.
#[derive(Debug, Clone)]
pub struct RollupPlan {
    pub sql: String,
    pub source: PreaggSource,
}

/// Reads a rollup. In production an in-memory DuckDB over its Parquet
/// ([`crate::preagg_exec::execute_rollup`]); a seam so tests need no Parquet.
#[async_trait]
pub trait RollupReader: Send + Sync {
    async fn read(&self, plan: RollupPlan, limit: u64) -> Result<ExecutionResult, String>;
}

/// The FSM's rollup executor.
struct DuckDbRollups;

#[async_trait]
impl RollupReader for DuckDbRollups {
    async fn read(&self, plan: RollupPlan, limit: u64) -> Result<ExecutionResult, String> {
        crate::preagg_exec::execute_rollup(plan.sql, plan.source, limit).await
    }
}

/// The platform's rollups, as `query` uses them.
#[derive(Clone)]
pub struct Rollups {
    context: PreaggContext,
    reader: Arc<dyn RollupReader>,
}

impl Rollups {
    pub fn new(context: PreaggContext) -> Self {
        Self::with_reader(context, Arc::new(DuckDbRollups))
    }

    pub fn with_reader(context: PreaggContext, reader: Arc<dyn RollupReader>) -> Self {
        Self { context, reader }
    }

    /// The live rollup that answers `request` (compiled to `warehouse_sql`),
    /// if one covers it and its SQL passes the read-only gate.
    pub(super) fn plan(
        &self,
        catalog: &SemanticCatalog,
        request: &QueryRequest,
        warehouse_sql: &str,
    ) -> Option<RollupPlan> {
        let live = Some(catalog.live_rollups());
        let resolved = try_resolve_preagg(&self.context, request, warehouse_sql, "", live)?;
        let plan = into_plan(resolved)?;
        if let Err(r) = gate(&plan.sql, sqlparser_dialect(SqlDialect::DuckDb).as_ref()) {
            tracing::warn!(
                error = r.message(),
                "preagg: the read-only gate refused the rollup SQL; using the warehouse"
            );
            return None;
        }
        Some(plan)
    }

    /// The rollup's rows, or `None` when it will not read: the caller falls
    /// back to the warehouse, which has the same answer, slower.
    pub(super) async fn read(&self, plan: RollupPlan, limit: u64) -> Option<ExecutionResult> {
        let remote = plan.source.is_remote();
        match self.reader.read(plan, limit).await {
            Ok(exec) => Some(exec),
            Err(error) => {
                tracing::warn!(
                    remote,
                    %error,
                    "preagg rollup read failed; falling back to the warehouse"
                );
                None
            }
        }
    }
}

/// The rollup half of a resolved query; `None` for the warehouse.
fn into_plan(compiled: CompiledQuery) -> Option<RollupPlan> {
    match compiled {
        CompiledQuery::Preaggregation {
            preagg_sql, source, ..
        } => Some(RollupPlan {
            sql: preagg_sql,
            source,
        }),
        CompiledQuery::Warehouse { .. } => None,
    }
}
