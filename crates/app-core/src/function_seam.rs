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
//! Lives in `oxy-app-core` because both sides of it are outside the custom-apps
//! crate: the data plane and `agentic_wiring` implement it, the function runtime
//! and the composition roots name it. `oxy-app` re-exports this module at
//! `custom_apps_functions::seam`.
//!
//! This is the ONE canonical path: gated and non-gated code alike import these
//! two traits from `seam::` — `runtime` (the V8 impl) does not re-export them, so
//! a new call site can't accidentally reach for a gated path and break the
//! feature-off build again.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use axum::http::{HeaderMap, Method, Uri};
use axum::response::Response;

use crate::preagg_cache_ctx::PreaggCacheCtx;

/// One `<method> /customer-apps/<org>/<app>/fn/<name>` request, as the serve
/// route hands it to the function runtime once it has split the path.
pub struct FunctionRequest<'a> {
    pub org_slug: &'a str,
    pub app_slug: &'a str,
    pub function_name: &'a str,
    pub method: Method,
    /// The URI the outer stack routed on, kept whole: `?refresh` is read from
    /// it, and a replica replays it to the Factory.
    pub uri: Uri,
    pub headers: HeaderMap,
    pub body: axum::body::Bytes,
    pub preagg: PreaggCacheCtx,
}

/// The function runtime's answer to a [`FunctionRequest`].
pub type FunctionResponse<'a> = Pin<Box<dyn Future<Output = Response> + Send + 'a>>;

/// Runs a function's read-only SQL against a warehouse `connector`, capped at
/// `max_rows` rows, returning the rows as JSON objects.
///
/// This seam owns the contract (the runtime consumes it); the production
/// implementation (`oxy_api_custom_apps::data_plane::query::DataPlaneQueryExecutor`) lives in the shared
/// data plane so the read-only gate and outer-LIMIT wrap stay shared with the
/// `/query` endpoint, and is injected at the composition root (the serve router
/// and the scheduled-function worker). Depending on this trait rather than
/// importing `data_plane::query` is what lets the custom-apps boundary test
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

    /// The project context a function invocation runs against, built from its
    /// workspace row: warehouse connectors, workspace config and the Airway
    /// seed ([`FunctionProjectContext`]). The production executor
    /// (`oxy_api_custom_apps::data_plane::query::DataPlaneQueryExecutor`) builds it from the data
    /// plane's own composition root (`custom_apps_gates`), so the runtime
    /// depends on this trait and not on `agentic_wiring`. The default refuses:
    /// an executor with no data plane behind it (a test fake) cannot build one.
    async fn build_project_context(
        &self,
        _workspace: &entity::workspaces::Model,
        _user_id: uuid::Uuid,
        _project_id: uuid::Uuid,
        _role: Option<entity::workspace_members::WorkspaceRole>,
    ) -> Result<Arc<dyn FunctionProjectContext>, ()> {
        tracing::error!(
            "custom-app function invoked through a FunctionQueryExecutor that cannot build a project context"
        );
        Err(())
    }
}

/// Runs a `/fn/<name>` request through the function runtime.
///
/// The runtime lives in `oxy-api-custom-apps`, above `oxy-app`; the one
/// `oxy-app` handler that runs a function on a person's behalf (a
/// notification's function action) reaches it through this object, which the
/// composition root hands in (`SurfaceSeams::function_runtime`) and the router
/// layers onto the API tree as an `Extension`.
pub trait FunctionRuntime: Send + Sync {
    /// Run one request exactly as `POST /customer-apps/<org>/<app>/fn/<name>`
    /// would: authentication, app access, the environment gate, the
    /// function's capability gates, its rate limit, the invocation row.
    fn handle<'a>(&'a self, request: FunctionRequest<'a>) -> FunctionResponse<'a>;
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

    /// An unscoped Reader for the app, which `ctx.airhouse.query` reads on
    /// under the `snapshot` mode: Airhouse refuses time travel on a
    /// schema-scoped Writer. It can write nothing. `schema` is the app's own,
    /// named for the audit subject. A context that cannot mint one refuses.
    async fn build_app_airhouse_reader(
        &self,
        app_slug: &str,
        schema: &str,
    ) -> Result<Arc<dyn agentic_connector::DatabaseConnector>, oxy_shared::errors::OxyError> {
        let _ = (app_slug, schema);
        Err(oxy_shared::errors::OxyError::ConfigurationError(
            "ctx.airhouse: this context cannot mint an app Reader".into(),
        ))
    }

    /// Seed an Airway run (`ctx.airway.run`) against `db`, returning the run id.
    async fn start_airway_seed(
        &self,
        db: &sea_orm::DatabaseConnection,
        request: agentic_pipeline::airway_run::StartAirwayRequest,
    ) -> Result<String, String>;
}
