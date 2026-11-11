//! Building a workspace's semantic layer, engine and query executor for one
//! panel query. Nothing here is pricing-specific; it is the recipe
//! `run_baseline_query` (`crates/app/src/server/api/metric_tree.rs`) uses,
//! minus the HTTP request: the executor through `oxy_app::surface::metric_tree_exec`,
//! the layer straight from the scan.

use std::sync::{Arc, LazyLock};

use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::ReadOnly;
use oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor;
use oxy_airlayer_compat::{DatabaseConfig, SemanticEngine, SemanticLayer};
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::panel::{PanelContext, PanelRefusal, check_revision};
use oxy_app::surface::SemanticEngineCacheCtx;
use oxy_app::surface::metric_tree_exec::{RunnerPreagg, build_query_executor, workspace_databases};
use oxy_app::surface::workspace_runtime::build_workspace_manager;
use oxy_app::surface::workspace_runtime::resolve_compiled_scan_source;
use oxy_app_core::workspace_cache::{SemanticEngineCache, new_semantic_engine_cache};

/// One cache for the process's price jobs. A worker has no `AppState` to carry
/// the request path's cache (see the note in `crates/server/src/commands/worker.rs`), and the
/// price job reads one promoted revision, which is exactly what the key names.
static ENGINE_CACHE: LazyLock<Arc<SemanticEngineCache>> = LazyLock::new(new_semantic_engine_cache);

pub(super) struct Opened {
    pub layer: SemanticLayer,
    engine: Arc<SemanticEngine>,
    databases: Vec<DatabaseConfig>,
    manager: WorkspaceManager<ReadOnly>,
}

impl Opened {
    /// The layer, and a warehouse executor for `user`, reading only (`Viewer`),
    /// with no rollup short-circuit: a placebo priced off a stale rollup would
    /// freeze the staleness into `mde_at_proposal`. Consumes `self` because the
    /// executor owns the workspace manager.
    pub(super) fn into_executor(
        self,
        user: Uuid,
        handle: tokio::runtime::Handle,
    ) -> (SemanticLayer, Box<QueryExecutor>) {
        let executor = build_query_executor(
            self.engine,
            self.databases,
            self.manager,
            user,
            WorkspaceRole::Viewer,
            handle,
            RunnerPreagg::default(),
            // A worker holds no request-scoped preview write hold; this job only reads.
            false,
        );
        (self.layer, executor)
    }
}

fn workspace_fault(what: &str, e: impl std::fmt::Display) -> OxyError {
    PanelRefusal::Workspace(format!("{what}: {e}")).into()
}

pub(super) async fn open(db: &DatabaseConnection, ctx: &PanelContext) -> Result<Opened, OxyError> {
    let manager = build_workspace_manager(db, ctx.workspace_id)
        .await
        .map_err(|e| workspace_fault("building the workspace", e))?;
    check_revision(manager.config_manager.revision_id(), ctx.revision_id)
        .map_err(OxyError::from)?;

    // The promoted revision only: pricing judges the workspace, it does not serve it.
    let scan = resolve_compiled_scan_source(&manager)
        .await
        .map_err(|e| workspace_fault("resolving the semantic model", e.message()))?;
    // `ScanDir`: the compiled model in memory or a directory on disk (#3833).
    let scan_dir = scan.scan.clone();
    let source_revision = scan.source_revision(&manager);
    let layer = tokio::task::spawn_blocking(move || scan_dir.load_layer())
        .await
        .map_err(|e| workspace_fault("loading the semantic model", e))?
        .map_err(|e| workspace_fault("loading the semantic model", format!("{e:?}")))?;
    drop(scan); // the layer is parsed; nothing reads the scan directory again

    let databases = workspace_databases(&manager);
    // Keyed by the revision the scan actually read, so the entry is immutable
    // content: no file write can make it stale, and a promote starts a new key.
    let engines = SemanticEngineCacheCtx {
        cache: ENGINE_CACHE.clone(),
        workspace_id: ctx.workspace_id,
    };
    let engine = engines
        .get_or_build(
            engines.scan_key(source_revision, &databases),
            Arc::new(layer.clone()),
            databases.clone(),
        )
        .await
        .map_err(|e| workspace_fault("building the semantic engine", e))?;
    Ok(Opened {
        layer,
        engine,
        databases,
        manager: manager.into_read_only(),
    })
}
