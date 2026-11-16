//! The metric tree's executor-facing core: what the IDE's metric-tree handlers
//! (`oxy-api-metric-monitoring`) and their customer-app twins
//! (`oxy-api-custom-apps`' `data_plane::metric_tree*`) share, and what neither
//! surface may hold for the other (S3: a surface never imports another).
//!
//! Beside [`super::metric_tree_runner`], whose executor it drives: the error the
//! analysis ops answer in, the scan resolution (compile boundary first, working
//! copy second), the layer and engine builders, the workspace's airlayer
//! database configs, the one instance-scope rule, and the projection's
//! validate-query-fit pipeline. Platform runtime, not metric-tree logic: the
//! computation itself lives in `oxy-metric-monitoring`. Surfaces reach it
//! through `oxy_app::surface::metric_tree_exec`.

use std::collections::HashMap;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::NaiveDate;
use serde::Deserialize;
use uuid::Uuid;

use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy_metric_monitoring::metric_tree_baseline::{
    BASELINE_TIMEOUT, PartialSplitExecutor, baseline_query_deadline, forward_reachable,
};
use oxy_metric_monitoring::metric_tree_projection::{
    MonitorSettings, ProjectionRequest, ProjectionResponse, SeriesByMeasure,
    parse_monitor_settings, project_one, projection_note, read_series, seasonality_for, validate,
};
use oxy_metric_monitoring::metric_tree_scenario::BaselineInstance;
use oxy_metric_monitoring::metric_tree_series::build_time_series_query_request;

use super::metric_tree_runner::{OxyMetricTreeRunner, RunnerPreagg, build_query_executor};
use crate::agentic_wiring::scan_source::{QueryScanSource, resolve_query_scan_source};
use crate::server::preagg_context::RollupFreshness;

#[derive(Debug)]
pub enum MetricTreeError {
    LayerLoad(String),
    /// No semantic model is reachable on this node: the workspace has no
    /// compiled revision and this replica has no working copy to fall back to.
    /// Retryable — a compile is enqueued on the way out — so it must not be
    /// flattened into the generic 500 above.
    ScanUnavailable(String),
    NotFound(String),
    /// The request itself is out of range — a caller error, not a failure.
    /// Distinct from [`MetricTreeError::Op`] because that one is logged as a
    /// server error and answered with a generic message, which is exactly
    /// wrong for something the caller can fix by changing a number.
    BadRequest(String),
    Op(String),
}

impl IntoResponse for MetricTreeError {
    fn into_response(self) -> Response {
        let mut retry_after = false;
        let (status, msg) = match &self {
            MetricTreeError::LayerLoad(e) => {
                tracing::error!("metric-tree layer load failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to load semantic model".to_string(),
                )
            }
            MetricTreeError::ScanUnavailable(m) => {
                // Logged, not silent: the "no compiled revision at all" branch
                // leaves no breadcrumb anywhere else (`resolve_query_scan_source`
                // only warns when a materialise actually failed), so a workspace
                // stuck un-compiled would be visible to the client and to nobody
                // operating the fleet.
                tracing::warn!("metric-tree scan unavailable: {m}");
                retry_after = true;
                (StatusCode::SERVICE_UNAVAILABLE, m.clone())
            }
            MetricTreeError::NotFound(m) => (StatusCode::NOT_FOUND, m.clone()),
            MetricTreeError::BadRequest(m) => (StatusCode::BAD_REQUEST, m.clone()),
            MetricTreeError::Op(e) => {
                tracing::error!("metric-tree op failed: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Metric tree operation failed".to_string(),
                )
            }
        };
        if retry_after {
            // A compile is enqueued on the way out; give the client a concrete
            // interval instead of leaving the retry cadence to guesswork.
            return (status, [(axum::http::header::RETRY_AFTER, "5")], msg).into_response();
        }
        (status, msg).into_response()
    }
}

/// A projection refused before any query runs, in this module's currency: an
/// unknown lever is a 404, an out-of-range request a 400.
impl From<oxy_metric_monitoring::metric_tree_projection::ProjectionRefusal> for MetricTreeError {
    fn from(r: oxy_metric_monitoring::metric_tree_projection::ProjectionRefusal) -> Self {
        use oxy_metric_monitoring::metric_tree_projection::ProjectionRefusal;
        match r {
            ProjectionRefusal::UnknownMeasure(m) => MetricTreeError::NotFound(m),
            ProjectionRefusal::BadRequest(m) => MetricTreeError::BadRequest(m),
        }
    }
}

/// Resolve the scan root for this request: compile boundary first, working copy
/// second — the same resolution `execute_semantic_query` performs.
///
pub async fn resolve_scan<S: oxy::config::DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
) -> Result<QueryScanSource, MetricTreeError> {
    resolve_query_scan_source(workspace_manager)
        .await
        .map_err(|e| MetricTreeError::ScanUnavailable(e.message()))
}

/// Parse the workspace's semantic model from an already-resolved scan.
pub fn load_layer_at(
    scan: &oxy::config::ScanDir,
) -> Result<oxy_airlayer_compat::SemanticLayer, MetricTreeError> {
    OxyMetricTreeRunner::load_layer_at(scan).map_err(|e| MetricTreeError::LayerLoad(e.to_string()))
}

/// The airlayer database configs for the workspace.
pub fn workspace_databases<S: oxy::config::DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
    OxyMetricTreeRunner::list_databases_sync(workspace_manager)
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    /// Optional measure id to root the returned subtree at.
    pub root: Option<String>,
}

/// Build the airlayer `SemanticEngine` for the workspace, with dialects
/// resolved from the configured databases.
pub fn build_engine(
    layer: oxy_airlayer_compat::SemanticLayer,
    databases: &[oxy_airlayer_compat::DatabaseConfig],
) -> Result<oxy_airlayer_compat::SemanticEngine, MetricTreeError> {
    oxy_airlayer_compat::build_engine(layer, databases)
        .map_err(|e| MetricTreeError::Op(e.to_string()))
}

/// The rule, once, for both surfaces.
///
/// The `projects/` twin needs the same decision in a different error currency
/// (a `Response`, not a `MetricTreeError`), so the rule lives here and each
/// caller supplies only the wrapping. Two copies of *this* is what a scope that
/// silently drops on one surface and errors on the other would look like.
///
/// Returns the message on failure rather than an error type, so neither
/// currency leaks into the other's module.
pub fn baseline_scope_core(
    layer: &oxy_airlayer_compat::SemanticLayer,
    instance: Option<&BaselineInstance>,
) -> Result<Vec<oxy_airlayer_compat::engine::query::QueryFilter>, String> {
    let Some(instance) = instance else {
        return Ok(Vec::new());
    };
    oxy_world_model::graph::instance_scope_filters(layer, &instance.entity, &instance.key)
        .ok_or_else(|| {
            format!(
                "cannot scope to '{}': no entity named '{}' in the semantic layer",
                instance.key, instance.entity
            )
        })
}

// ── Projection ─────────────────────────────────────────────────────────────

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
pub struct ProjectionExec {
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
pub struct ProjectionOutcome {
    pub response: ProjectionResponse,
    pub query_failed: bool,
}

/// Validate, query, fit — the whole projection, independent of which surface
/// asked for it. See [`ProjectionExec`] for what the surfaces supply.
pub async fn run_projection(
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
    let preagg = RunnerPreagg {
        cache: preagg_cache,
        renewal_threshold_secs: preagg_renewal_threshold_secs,
        // A read surface: a projection is a curve drawn from the data, so a
        // rollup a cycle behind is a late point on a chart, never an assertion.
        freshness: RollupFreshness::ServeStale,
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
