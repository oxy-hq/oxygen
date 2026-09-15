//! Dependency-inversion seam for custom-app functions — always compiled.
//!
//! These two traits are the ABSTRACT contract between the function runtime and
//! the rest of the app. They live here, independent of the `custom-app-functions`
//! feature, so non-gated call sites (the serve-router wiring, the
//! `OxyProjectContext` adapter, the data-plane executor) can name them even when
//! the V8 runtime (`runtime.rs`) is compiled out. Only the concrete V8
//! implementation is feature-gated; the abstract seam is not — otherwise
//! `--no-default-features` fails to compile (the trait names vanish under
//! non-gated `use`s).
//!
//! This is the ONE canonical path: gated and non-gated code alike import these
//! two traits from `seam::` — `runtime` (the V8 impl) does not re-export them, so
//! a new call site can't accidentally reach for a gated path and break the
//! feature-off build again.

use std::sync::Arc;

/// Runs a function's read-only SQL against a warehouse `connector`, capped at
/// `max_rows` rows, returning the rows as JSON objects.
///
/// This seam owns the contract (the runtime consumes it); the production
/// implementation (`projects::query::DataPlaneQueryExecutor`) lives in the shared
/// data plane so the read-only gate and outer-LIMIT wrap stay shared with the
/// `/query` endpoint, and is injected at the composition root (the serve router
/// and the scheduled-function worker). Depending on this trait rather than
/// importing `projects::query` is what lets the custom-apps boundary test
/// (`tests/custom_apps/custom_apps_boundary.rs`) drop that seam — and lets a test drive the
/// runtime with a fake executor.
#[async_trait::async_trait]
pub trait FunctionQueryExecutor: Send + Sync {
    async fn execute(
        &self,
        connector: Arc<dyn agentic_connector::DatabaseConnector>,
        sql: &str,
        max_rows: usize,
    ) -> Result<Vec<serde_json::Value>, String>;
}

/// The per-invocation project context a function runs against — warehouse
/// connectors, workspace config, and the Airway seed. This seam owns the
/// contract (the runtime consumes it); the production impl is `agentic_wiring::OxyProjectContext` (named
/// outside the custom-apps boundary), so the runtime depends on the trait
/// rather than on `agentic_wiring`. Injected at `ProjectFunctionHost`
/// construction, exactly like [`FunctionQueryExecutor`].
///
/// `start_airway_seed` wraps `start_airway_run` so the pipeline's
/// `WorkspaceContext` requirement stays behind the impl, off the boundary.
#[async_trait::async_trait]
pub trait FunctionProjectContext: Send + Sync {
    /// The workspace manager — read `config_manager` for the configured
    /// databases, the default database, and the semantics scan path.
    fn workspace_manager(
        &self,
    ) -> &oxy::adapters::workspace::manager::WorkspaceManager<oxy::config::WorkingCopy>;

    /// Build a warehouse connector for `db_name`.
    async fn build_connector_for(
        &self,
        db_name: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, oxy_shared::errors::OxyError>;

    /// The Airhouse connection `ctx.airhouse` writes an app's own facts through:
    /// minted for the app whoever invoked it, and asked to be confined to
    /// `schema`. Every statement is still checked host-side first.
    async fn build_app_airhouse_connector(
        &self,
        app_slug: &str,
        schema: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, oxy_shared::errors::OxyError>;

    /// Seed an Airway run (`ctx.airway.run`) against `db`, returning the run id.
    async fn start_airway_seed(
        &self,
        db: &sea_orm::DatabaseConnection,
        request: agentic_pipeline::airway_run::StartAirwayRequest,
    ) -> Result<String, String>;
}
