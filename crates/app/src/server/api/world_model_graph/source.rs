//! Where the world-model handlers read the semantic model from, and which
//! queries a pod with no working copy must refuse.
//!
//! Every route in this module is `FleetOk`, so none may read
//! `semantics_scan_path()` — the working copy — directly: a `serve` replica has
//! none. The scan root comes from [`resolve_query_scan_source`], the resolution
//! the metric tree and `execute_semantic_query` use: the compiled revision,
//! materialised, on a pod with no files; the working copy on a node that owns
//! them.

use std::sync::Arc;

use axum::{extract::Json, http::StatusCode};
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy::config::DiskSlot;
use oxy_airlayer_compat::engine::promotions::Promotions;
use uuid::Uuid;

use crate::server::api::middlewares::workspace_context::{
    SemanticEngineCacheCtx, SemanticLayerCacheCtx,
};
use crate::server::api::semantic::{
    ErrorResponse, QueryScanSource, resolve_query_scan_source, semantic_err,
};
use crate::server::serve_safety;

/// The error every handler here answers with.
pub(super) type WmError = (StatusCode, Json<ErrorResponse>);

/// The parsed model, its promotion closure, and where it was read from.
pub(super) type LoadedModel = (
    Arc<oxy_airlayer_compat::SemanticLayer>,
    Promotions,
    ModelSource,
);

/// The scan root one request reads, and the revision it was read from.
///
/// Owns the materialised tempdir, so it must outlive the parse of the layer.
/// Nothing after that reads the directory: every compile in this module is
/// handed the parsed layer, or an engine built from it.
pub(super) struct ModelSource {
    scan: QueryScanSource,
    /// `Some` only when the scan materialised a compiled revision; `None` for
    /// the working copy. The one value both caches are keyed by.
    revision: Option<Uuid>,
}

impl ModelSource {
    /// Compile boundary first, working copy second. A pod that has neither
    /// answers a retryable 503 with a compile already enqueued — never an
    /// empty model, which would read as "this workspace models nothing".
    pub(super) async fn resolve<S: DiskSlot>(
        workspace_manager: &WorkspaceManager<S>,
    ) -> Result<Self, WmError> {
        let scan = resolve_query_scan_source(workspace_manager)
            .await
            .map_err(|e| semantic_err(StatusCode::SERVICE_UNAVAILABLE, e.message()))?;
        let revision = scan.source_revision(workspace_manager);
        Ok(Self { scan, revision })
    }

    pub(super) fn scan_path(&self) -> std::path::PathBuf {
        self.scan.path().to_path_buf()
    }

    /// The parsed model, from the per-workspace layer cache.
    pub(super) async fn layer(
        &self,
        layer_cache: &SemanticLayerCacheCtx,
    ) -> Result<Arc<oxy_airlayer_compat::SemanticLayer>, oxy_airlayer_compat::SemanticError> {
        layer_cache
            .get_or_load(self.revision, self.scan_path())
            .await
    }

    /// The engine-cache key for the model this request read.
    pub(super) fn engine_key(
        &self,
        engine_cache: &SemanticEngineCacheCtx,
        databases: &[oxy_airlayer_compat::DatabaseConfig],
    ) -> oxy_airlayer_compat::EngineKey {
        engine_cache.scan_key(self.revision, databases)
    }
}

fn internal(e: impl std::fmt::Display) -> WmError {
    semantic_err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Load the semantic model and build its promotion closure — the preamble the
/// world-model handlers otherwise repeat verbatim (resolve the scan →
/// `get_or_load` → `Promotions::build`). Returns the transport error tuple
/// ready to `?`-propagate from a handler so the load path lives in one place.
///
/// The [`ModelSource`] comes back with it: the caller keys its engine by what
/// the scan read.
pub(super) async fn load_layer_and_promotions<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    layer_cache: &SemanticLayerCacheCtx,
) -> Result<LoadedModel, WmError> {
    let source = ModelSource::resolve(workspace_manager).await?;
    let layer = source.layer(layer_cache).await.map_err(internal)?;
    let promotions = Promotions::build(&layer.views).map_err(internal)?;
    Ok((layer, promotions, source))
}

/// [`load_layer_and_promotions`] for a handler that walks the graph and so
/// queries every entity's database: the filter walk and the per-entity counts.
/// Those report a node whose query failed as empty, so a database this pod
/// cannot open has to be refused before the walk rather than discovered in it.
///
/// Conservative on purpose: every view's database is in scope, though a walk
/// from one seed may never reach some of them. The set a walk reaches is not
/// known until the walk has run, and by then a node that could not be queried
/// has already been reported as empty. Narrowing this to "the views this seed
/// reaches" brings that back.
pub(super) async fn load_walkable_model<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    layer_cache: &SemanticLayerCacheCtx,
) -> Result<LoadedModel, WmError> {
    let loaded = load_layer_and_promotions(workspace_manager, layer_cache).await?;
    let every_view = loaded.0.views.iter().map(|v| v.datasource.as_deref());
    refuse_databases_this_pod_cannot_open(workspace_manager, every_view)?;
    Ok(loaded)
}

/// Refuse, on a pod that holds no working copy, a query against a database
/// that is a file in it — a local DuckDB with no S3 mirror, a key file.
///
/// The handlers here report a failed warehouse query as an empty page or a
/// zero count. That is right for one unreachable node in a graph, and wrong
/// for a pod that cannot open the database at all: the panel would read "no
/// instances" for an entity that has thousands. So the pod names the database
/// it cannot open instead. A node that owns the files never asks.
///
/// `datasources` are the `datasource:` of each view the request will query;
/// `None` is a view that names none, which may resolve to any database and so
/// puts every configured one in scope.
pub(super) fn refuse_databases_this_pod_cannot_open<'a, S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    datasources: impl IntoIterator<Item = Option<&'a str>>,
) -> Result<(), WmError> {
    if oxy::workspace_fs_probe::process_owns_workspace_files() {
        return Ok(());
    }
    let databases = workspace_manager.config_manager.list_databases();
    match serve_safety::first_needing_working_copy(&databases, datasources) {
        None => Ok(()),
        Some(name) => Err(semantic_err(
            StatusCode::SERVICE_UNAVAILABLE,
            format!(
                "database `{name}` is a file in the workspace working copy (a local DuckDB \
                 with no S3 mirror, or a key file), which this instance does not have; only \
                 the instance that holds the working copy can query it"
            ),
        )),
    }
}

/// [`refuse_databases_this_pod_cannot_open`] for the one database `entity`'s
/// primary view reads — the instance picker queries nothing else. An unknown
/// entity is the caller's 404 to give.
pub(super) fn refuse_entity_database<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    layer: &oxy_airlayer_compat::SemanticLayer,
    entity: &str,
) -> Result<(), WmError> {
    match super::query::primary_view_of(layer, entity) {
        Some(view) => {
            refuse_databases_this_pod_cannot_open(workspace_manager, [view.datasource.as_deref()])
        }
        None => Ok(()),
    }
}

/// [`load_layer_and_promotions`] for a handler that queries the one database
/// `entity`'s primary view reads, and builds that connector before it streams.
pub(super) async fn load_entity_model<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    layer_cache: &SemanticLayerCacheCtx,
    entity: &str,
) -> Result<LoadedModel, WmError> {
    let loaded = load_layer_and_promotions(workspace_manager, layer_cache).await?;
    refuse_entity_database(workspace_manager, &loaded.0, entity)?;
    Ok(loaded)
}

/// The one database a measure breakdown queries: the request's `datasource`
/// when it names one, else the entity's primary view's.
pub(super) fn refuse_breakdown_database<S: DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
    layer: &oxy_airlayer_compat::SemanticLayer,
    q: &super::types::WmMeasureBreakdownQuery,
) -> Result<(), WmError> {
    match q.datasource.as_deref() {
        Some(name) => refuse_databases_this_pod_cannot_open(workspace_manager, [Some(name)]),
        None => refuse_entity_database(workspace_manager, layer, &q.entity),
    }
}

#[cfg(test)]
#[path = "source_tests.rs"]
mod tests;
