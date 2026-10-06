//! The dry run: what the next scan would make of one `.monitor.yml` entry,
//! with nothing written — no anomaly rows, no run, no Slack post.
//!
//! `scan` is the only other way to find that out, and it answers by filing
//! into the Insights Inbox. This is for the author who wants the answer first.
//!
//! Fleet-safe on purpose, unlike `scan`. The file is read the way
//! `list_monitors` reads it — the compile boundary, or the working copy on a
//! node that holds one — so the entry previewed is the entry the tab listed,
//! and the semantic model through `resolve_scan`. The handler asks for no
//! disk and any replica can serve it. The detection itself is
//! `oxy_metric_monitoring::preview`.

use agentic_http::AgenticState;
use axum::extract::{Extension, Json};
use chrono::Utc;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_metric_monitoring as monitoring;
use oxy_metric_monitoring::preview::{MonitorPreview, MonitorSelector};
use std::sync::Arc;
use std::time::Duration;

use super::error::AnomalyError;
use crate::agentic_wiring::OxyMetricTreeRunner;
use crate::server::api::metric_tree::resolve_scan;
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, WorkspaceManagerReadOnly,
};

/// Inside the 60-second request timeout, so a slow warehouse gets this
/// handler's sentence rather than a bare gateway error.
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(50);

/// `POST /workspaces/{workspace_id}/semantic/anomalies/preview` — scan the one
/// monitor the body names and return what it found.
pub async fn preview_monitor(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    Extension(state): Extension<Arc<AgenticState>>,
    preagg_ctx: PreaggCacheCtx,
    Json(selector): Json<MonitorSelector>,
) -> Result<Json<MonitorPreview>, AnomalyError> {
    let workspace_id = workspace_manager.workspace_id;
    let config_manager = workspace_manager.config_manager.clone();
    let definition = config_manager
        .monitor_config()
        .await
        .map_err(|e| match e.retryable() {
            true => AnomalyError::Unavailable(e.to_string()),
            false => AnomalyError::BadRequest(e.to_string()),
        })?
        .ok_or(AnomalyError::NoSuchMonitor)?;
    let config = monitoring::from_definition(definition)
        .map_err(|e| AnomalyError::BadRequest(e.to_string()))?;
    let entry = selector.find(&config).ok_or(AnomalyError::NoSuchMonitor)?;

    // `source` owns the materialised semantic model; it has to outlive the run.
    let source = resolve_scan(&workspace_manager)
        .await
        .map_err(|_| AnomalyError::Unavailable("the semantic model is not readable yet".into()))?;
    // The same rollup rule the scan runs under, so the two cannot disagree: a
    // rollup whose newest buckets are missing would read as a drop.
    let runner = Arc::new(
        OxyMetricTreeRunner::new(workspace_manager, user.id, role)
            .with_scan_path(source.scan_path.clone())
            .with_preagg(
                preagg_ctx.cache.clone(),
                preagg_ctx.renewal_threshold_secs_or(&config_manager),
            )
            .requiring_fresh_rollups(),
    );
    let open_events = monitoring::load_open_events(&state.db, workspace_id)
        .await
        .map_err(AnomalyError::Db)?;

    let run = monitoring::preview::preview_monitor(runner, entry, Utc::now(), &open_events);
    let preview = tokio::time::timeout(PREVIEW_TIMEOUT, run)
        .await
        .map_err(|_| AnomalyError::TimedOut(PREVIEW_TIMEOUT.as_secs()))?
        .map_err(AnomalyError::Scan)?;
    Ok(Json(preview))
}
