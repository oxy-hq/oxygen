//! `POST .../semantic/metric-tree/projection` — history and a forward curve for
//! the scenario canvas's levers and everything downstream of them.
//!
//! The host half: resolve the scan, read `.monitor.yml` through the compile
//! boundary, build the executor from Oxy's connector pool, and run the query
//! and the fits off the async runtime. The pure half — the wire shapes,
//! validation, seasonality resolution, the per-view read and the fit — is
//! `oxy_metric_monitoring::metric_tree_projection`, re-exported below.

use axum::extract::Json;
use oxy_auth::extractor::AuthenticatedUserExtractor;

use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_server_authz::workspace_role::EffectiveWorkspaceRole;
use oxy_workspace_runtime::metric_tree_exec::{load_layer_at, resolve_scan};
use oxy_workspace_runtime::request::workspace_context::WorkspaceManagerReadOnly;

// The executor-facing core both surfaces share (`ProjectionExec`,
// `run_projection`, the error) is `oxy_workspace_runtime::metric_tree_exec`; the pure
// half — wire shapes, validation, seasonality, the per-view read and the fit —
// lives in `oxy-metric-monitoring`. Both re-exported so every
// `metric_tree_projection::` path holds.
pub use oxy_metric_monitoring::metric_tree_projection::{
    ForecastPoint, HistoryPoint, MAX_PROJECTION_HORIZON, MeasureProjection, ProjectionRefusal,
    ProjectionRequest, ProjectionResponse, parse_series,
};
pub use oxy_workspace_runtime::metric_tree_exec::{
    MetricTreeError, ProjectionExec, ProjectionOutcome, run_projection,
};

/// `POST .../semantic/metric-tree/projection` — history and forward curve for
/// the levers and everything downstream of them.
pub async fn post_projection(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    preagg_ctx: PreaggCacheCtx,
    Json(req): Json<ProjectionRequest>,
) -> Result<Json<ProjectionResponse>, MetricTreeError> {
    // The scan source owns a materialised tempdir; hold it until the
    // projection has finished every read of `scan_path`.
    let source = resolve_scan(&workspace_manager).await?;
    let layer = load_layer_at(&source.scan)?;
    let exec = ProjectionExec {
        preagg_cache: preagg_ctx.cache.clone(),
        preagg_renewal_threshold_secs: preagg_ctx
            .renewal_threshold_secs_or(&workspace_manager.config_manager),
        workspace_manager,
        user_id: user.id,
        role,
    };
    Ok(Json(run_projection(exec, layer, req).await?.response))
}
