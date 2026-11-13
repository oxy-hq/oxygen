//! `POST .../semantic/metric-tree/projection` — history and a forward curve for
//! the scenario canvas's levers and everything downstream of them.
//!
//! The host half: resolve the scan, read `.monitor.yml` through the compile
//! boundary, build the executor from Oxy's connector pool, and run the query
//! and the fits off the async runtime. The pure half — the wire shapes,
//! validation, seasonality resolution, the per-view read and the fit — is
//! `oxy_metric_monitoring::metric_tree_projection`, re-exported below.

use std::collections::HashMap;

use axum::extract::Json;
use chrono::NaiveDate;
use uuid::Uuid;

use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy_auth::extractor::AuthenticatedUserExtractor;

use crate::agentic_wiring::metric_tree_runner::build_query_executor;
use crate::server::api::metric_tree::{
    BASELINE_TIMEOUT, MetricTreeError, baseline_query_deadline, baseline_scope_core, build_engine,
    load_layer_at, resolve_scan, workspace_databases,
};
use crate::server::api::metric_tree_baseline::{PartialSplitExecutor, forward_reachable};
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, WorkspaceManagerReadOnly,
};
use oxy_metric_monitoring::metric_tree_series::build_time_series_query_request;

// The pure half — wire shapes, validation, seasonality, the per-view read and
// the fit — lives in `oxy-metric-monitoring`; re-exported so every
// `server::api::metric_tree_projection::` path holds.
pub use oxy_metric_monitoring::metric_tree_projection::{
    ForecastPoint, HistoryPoint, MAX_PROJECTION_HORIZON, MeasureProjection, ProjectionRefusal,
    ProjectionRequest, ProjectionResponse, parse_series,
};
use oxy_metric_monitoring::metric_tree_projection::{
    MonitorSettings, SeriesByMeasure, parse_monitor_settings, project_one, projection_note,
    read_series, seasonality_for, validate,
};

/// Everything a projection needs to reach the warehouse, gathered by whichever
/// surface owns the request.
///
/// The two callers differ only in these six fields: the IDE reads the
/// workspace's own scan path, the caller's role and the pre-aggregation cache
/// the middleware attached, while the customer-app surface pins the layer to
/// the compile-boundary tempdir and enters read-only with no pre-agg cache of
/// its own. Everything downstream of here — validation, seasonality
/// resolution, the query, the fit — is one code path, which is what stops the
/// SDK's curves from drifting from the canvas's.
pub(crate) struct ProjectionExec {
    pub workspace_manager: WorkspaceManager<oxy::config::ReadOnly>,
    pub user_id: Uuid,
    pub role: WorkspaceRole,
    /// Semantic scan path the executor resolves views against.
    pub preagg_cache: Option<std::sync::Arc<std::sync::RwLock<RefreshKeyCache>>>,
    pub preagg_renewal_threshold_secs: u64,
}

/// A projection, plus whether the warehouse refused any part of it.
///
/// The flag is separate from the response because it is not a fact about the
/// workspace: a caller that caches must not remember a warehouse that happened
/// to be down. The refusals still *ship* — each one is on its own measure, and
/// a panel drawing three working curves must say why the fourth is missing.
pub(crate) struct ProjectionOutcome {
    pub response: ProjectionResponse,
    pub query_failed: bool,
}

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

/// Validate, query, fit — the whole projection, independent of which surface
/// asked for it. See [`ProjectionExec`] for what the surfaces supply.
pub(crate) async fn run_projection(
    exec: ProjectionExec,
    layer: oxy_airlayer_compat::SemanticLayer,
    req: ProjectionRequest,
) -> Result<ProjectionOutcome, MetricTreeError> {
    let tree = oxy_semantic::build_metric_tree(&layer);
    validate(&tree, &req)?;
    let scope =
        baseline_scope_core(&layer, req.instance.as_ref()).map_err(MetricTreeError::NotFound)?;
    let measures = forward_reachable(&tree, &req.roots);
    // One read of `.monitor.yml`, compiled row first, for BOTH things this
    // projection takes from it: the file-level `timezone` the query buckets in
    // and the entries `seasonality_for` resolves against. Unconditional even
    // when the request pins its own periods, because the timezone is needed
    // regardless — and because this route is FleetOk, so the answer must not
    // depend on which replica holds a working copy.
    let MonitorSettings { timezone, monitors } =
        load_monitor_settings(&exec.workspace_manager).await;

    let (history, failures) =
        run_projection_query(exec, layer, &req, scope, &measures, timezone).await?;

    // Fitting is CPU work and there is one fit per measure, so it goes off the
    // async runtime and across cores. An MSTL fit is ~5 ms, which is tolerable
    // inline for one measure and is not for twenty on a blocked runtime thread
    // against a 30-second request budget. Independent per measure, so the
    // parallelism is free.
    let fit_inputs: Vec<(String, Vec<(NaiveDate, f64)>, Option<String>, Vec<usize>)> = measures
        .iter()
        .map(|measure| {
            (
                measure.clone(),
                history.get(measure).cloned().unwrap_or_default(),
                failures.get(measure).cloned(),
                seasonality_for(&req, &monitors, measure),
            )
        })
        .collect();
    let granularity = req.granularity;
    let horizon = req.horizon;
    let series = tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;
        fit_inputs
            .into_par_iter()
            .map(|(measure, series, failure, seasonality)| {
                project_one(
                    &measure,
                    &series,
                    failure.as_deref(),
                    granularity,
                    horizon,
                    seasonality,
                )
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| MetricTreeError::BadRequest(format!("projection fit panicked: {e}")))?;

    Ok(ProjectionOutcome {
        query_failed: !failures.is_empty(),
        response: ProjectionResponse {
            granularity: req.granularity.airlayer_str().to_string(),
            resolved_period: req.period,
            horizon: req.horizon,
            series,
            projection_note: projection_note(&history, &failures, &measures),
        },
    })
}

/// The workspace's monitor settings, or the defaults (UTC, no entries).
///
/// Compiled row first so the resolution works on a stateless replica, with the
/// working copy as the fallback for a workspace that has not been promoted
/// yet — `monitor_config` does that split at the compile boundary, so this
/// function does not spell it out. Deliberately lossy: a malformed
/// `.monitor.yml` is the scan path's error to report, and must not cost an
/// unrelated projection its curves. Losing it costs UTC bucketing and the
/// granularity-default seasonality, which is what this endpoint used before
/// it read the file at all.
async fn load_monitor_settings(
    workspace_manager: &WorkspaceManager<oxy::config::ReadOnly>,
) -> MonitorSettings {
    let workspace_id = workspace_manager.workspace_id;
    let definition = match workspace_manager.config_manager.monitor_config().await {
        Ok(Some(definition)) => definition,
        Ok(None) => return MonitorSettings::default(),
        Err(e) => {
            tracing::debug!(
                target: "metric_monitoring",
                workspace_id = %workspace_id,
                error = ?e,
                "projection: could not read .monitor.yml; bucketing in UTC and falling \
                 back to the granularity-default seasonality"
            );
            return MonitorSettings::default();
        }
    };
    parse_monitor_settings(definition).unwrap_or_else(|e| {
        tracing::debug!(
            target: "metric_monitoring",
            workspace_id = %workspace_id,
            error = ?e,
            "projection: monitor config did not deserialise; bucketing in UTC and falling \
             back to the granularity-default seasonality"
        );
        MonitorSettings::default()
    })
}

/// Build the executor and run the bucketed query, off the async runtime.
///
/// Mirrors the baseline's `run_baseline_query`, including the split-retry
/// wrapper: this request carries a time dimension, so a tree mixing additive
/// and non-additive measures from one view hits the same fan-out refusal the
/// baseline does, and for the same reason must be retried as unmixed halves
/// rather than surfaced.
///
/// Returns the series it got and, separately, the measures it could not fetch
/// and why. Two maps rather than one result, because a projection is a panel of
/// independent curves: one measure the warehouse refuses must cost that
/// measure its curve and nothing else.
async fn run_projection_query(
    exec: ProjectionExec,
    layer: oxy_airlayer_compat::SemanticLayer,
    req: &ProjectionRequest,
    scope: Vec<oxy_airlayer_compat::engine::query::QueryFilter>,
    measures: &[String],
    timezone: Option<String>,
) -> Result<(SeriesByMeasure, HashMap<String, String>), MetricTreeError> {
    let ProjectionExec {
        workspace_manager,
        user_id,
        role,
        preagg_cache,
        preagg_renewal_threshold_secs,
    } = exec;
    let databases = workspace_databases(&workspace_manager);
    let engine = std::sync::Arc::new(build_engine(layer.clone(), &databases)?);
    let handle = tokio::runtime::Handle::current();
    // Read here: the executor builds its connectors on a blocking thread.
    let holds_writes = oxy_app_core::write_hold::active();
    let preagg = crate::agentic_wiring::metric_tree_runner::RunnerPreagg {
        cache: preagg_cache,
        renewal_threshold_secs: preagg_renewal_threshold_secs,
        // A read surface: a projection is a curve drawn from the data, so a
        // rollup a cycle behind is a late point on a chart, never an assertion.
        freshness: crate::server::preagg_context::RollupFreshness::ServeStale,
    };
    let granularity = req.granularity.airlayer_str().to_string();
    let (request, trim) = build_time_series_query_request(
        measures,
        &req.time_dimension,
        &granularity,
        scope,
        &req.period,
        timezone,
    );
    let dim_alias = format!("{}.{granularity}", req.time_dimension).replace('.', "__");
    let measures = measures.to_vec();

    tokio::time::timeout(
        BASELINE_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let executor = build_query_executor(
                engine,
                databases,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg,
                holds_writes,
            );
            // ONE deadline shared by the batched query, the split's groups and
            // `read_series`'s own per-view retries — see
            // `PartialSplitExecutor::out_of_budget`. Taken from
            // `baseline_query_deadline` rather than recomputed here: this file
            // used to carry its own copy of the fraction, which is how the two
            // surfaces would have drifted the moment either was tuned.
            let deadline = baseline_query_deadline();
            let executor = PartialSplitExecutor::new(layer, executor, deadline);
            read_series(&executor, &request, &measures, &dim_alias, trim)
        }),
    )
    .await
    .map_err(|_| {
        MetricTreeError::Op(format!(
            "projection timed out after {}s — consider a shorter period or a coarser granularity",
            BASELINE_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| MetricTreeError::Op(format!("projection task panicked: {e}")))
}
