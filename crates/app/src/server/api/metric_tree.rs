//! Metric Tree API — tree structure and analysis ops over the semantic model.
//!
//! The tree is built per request from the workspace's semantic model, resolved
//! through the same scan source `execute_semantic_query` uses —
//! [`resolve_query_scan_source`]: the compile boundary first, the working copy
//! second. Every route here is `FleetOk`, so reading the working copy directly
//! is not an option: a stateless serve replica has none, and scanning the
//! missing directory failed every call with a flat 500 (oxy-hq/oxygen#878).
//!
//! The pure ops — tree, `sensitivity`, `predict` — need no database access.
//! The query-executing ops (`explain`, `opportunity`) run airlayer's
//! algorithms against a `QueryExecutor` bridged to Oxy's connector: airlayer
//! compiles each `QueryRequest` to SQL, and `run_via_agentic_connector`
//! executes it.

use axum::{
    extract::{Json, Path, Query},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use uuid::Uuid;

use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy_airlayer_compat::engine::metric_tree::MetricTree;
use oxy_airlayer_compat::engine::metric_tree_fit::apply_fitted_coefficients;
use oxy_airlayer_compat::engine::metric_tree_ops::{ExplainConfig, ExplainResult};
use oxy_auth::extractor::AuthenticatedUserExtractor;

use crate::agentic_wiring::metric_tree_runner::{
    OxyMetricTreeRunner, build_drill_query_executor, build_query_executor,
};
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, SemanticEngineCacheCtx, WorkspaceManagerReadOnly,
};
use crate::server::api::semantic::{QueryScanSource, resolve_query_scan_source};

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
pub(crate) async fn resolve_scan<S: oxy::config::DiskSlot>(
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

/// Build the metric tree from an already-resolved scan.
fn load_tree_at(scan: &oxy::config::ScanDir) -> Result<MetricTree, MetricTreeError> {
    let layer = load_layer_at(scan)?;
    Ok(oxy_semantic::build_metric_tree(&layer))
}

/// The airlayer database configs for the workspace.
pub fn workspace_databases<S: oxy::config::DiskSlot>(
    workspace_manager: &WorkspaceManager<S>,
) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
    OxyMetricTreeRunner::list_databases_sync(workspace_manager)
}

// ── Pure ops: tree, sensitivity, predict ────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    /// Optional measure id to root the returned subtree at.
    pub root: Option<String>,
}

/// `GET .../semantic/metric-tree` — the full tree, or `?root=<id>` subtree.
pub async fn get_metric_tree(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Query(q): Query<TreeQuery>,
) -> Result<Json<MetricTree>, MetricTreeError> {
    let source = resolve_scan(&workspace_manager).await?;
    let tree = load_tree_at(&source.scan)?;
    match q.root {
        Some(root) => oxy_semantic::subtree(&tree, &root)
            .map(Json)
            .ok_or_else(|| MetricTreeError::NotFound(format!("measure '{root}' not in tree"))),
        None => Ok(Json(tree)),
    }
}

/// `GET .../semantic/metric-tree/{measure_id}/sensitivity` — ranked drivers.
pub async fn get_sensitivity(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Path((_workspace_id, measure_id)): Path<(Uuid, String)>,
) -> Result<Json<oxy_airlayer_compat::engine::metric_tree_ops::SensitivityResult>, MetricTreeError>
{
    let source = resolve_scan(&workspace_manager).await?;
    let tree = load_tree_at(&source.scan)?;
    oxy_semantic::sensitivity(&tree, &measure_id)
        .map(Json)
        .map_err(|e| MetricTreeError::Op(e.to_string()))
}

/// `POST .../semantic/metric-tree/predict` — propagate hypothetical deltas.
pub async fn post_predict(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    Json(req): Json<PredictRequest>,
) -> Result<Json<oxy_airlayer_compat::engine::metric_tree_ops::PredictResult>, MetricTreeError> {
    let source = resolve_scan(&workspace_manager).await?;
    let layer = load_layer_at(&source.scan)?;
    let mut tree = oxy_semantic::build_metric_tree(&layer);
    // Before propagation: a fitted coefficient has to make the edge behave
    // exactly as a declared one would, or the forecast the user sees would
    // depend on where the number came from.
    apply_fitted_coefficients(&mut tree, &req.coefficients);
    let changes: Vec<(String, f64)> = req
        .changes
        .into_iter()
        .map(|c| (c.measure, c.delta))
        .collect();
    reject_lever_conflicts(&tree, &changes).map_err(MetricTreeError::BadRequest)?;
    match req.values {
        Some(values) => {
            let dialect = predict_dialect(
                &layer,
                &workspace_databases(&workspace_manager),
                &tree,
                &changes,
            );
            oxy_semantic::predict_with_values(&tree, &changes, &values, dialect)
        }
        None => oxy_semantic::predict(&tree, &changes),
    }
    .map(Json)
    .map_err(|e| MetricTreeError::Op(e.to_string()))
}

// ── Executor bridge ─────────────────────────────────────────────────────────
//
// The executor closure that compiles airlayer `QueryRequest`s and runs them
// through Oxy's connector pool lives in [`crate::agentic_wiring::metric_tree_runner`]
// so both this HTTP handler and the agentic analytics tools route through the
// same code path. See `build_query_executor` there.

/// Build the airlayer `SemanticEngine` for the workspace, with dialects
/// resolved from the configured databases.
pub(crate) fn build_engine(
    layer: oxy_airlayer_compat::SemanticLayer,
    databases: &[oxy_airlayer_compat::DatabaseConfig],
) -> Result<oxy_airlayer_compat::SemanticEngine, MetricTreeError> {
    oxy_airlayer_compat::build_engine(layer, databases)
        .map_err(|e| MetricTreeError::Op(e.to_string()))
}

/// Soft cap on how long an explain call can run before the handler bails.
/// Explain can fire 50+ warehouse queries on rich schemas; the UI just
/// shows a spinner the whole time, so failing loud after the cap is
/// kinder than hanging indefinitely.
const EXPLAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);

/// `POST .../semantic/metric-tree/explain` — period-over-period root cause.
///
/// Goes through `OxyMetricTreeRunner::run_explain` so the HTTP path and
/// the agentic path apply the same dim-pruning policy (the runner strips
/// high-cardinality numeric dims before passing the layer to airlayer).
pub async fn post_explain(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    preagg_ctx: PreaggCacheCtx,
    engine_cache: SemanticEngineCacheCtx,
    Json(req): Json<ExplainRequest>,
) -> Result<Json<ExplainResult>, MetricTreeError> {
    use agentic_analytics::MetricTreeRunner as _;
    // `source` owns the materialised tempdir — it must outlive `runner`, which
    // re-parses the layer from `scan_path` once per run it performs (in the
    // async `snapshot_for_blocking`, not inside the blocking task).
    let source = resolve_scan(&workspace_manager).await?;
    let config_manager = workspace_manager.config_manager.clone();
    // Resolved before the manager moves into the runner. What `source` actually
    // READ, not the request's pinned revision — those differ on a node that
    // holds a working copy and is pinned to a revision.
    let source_revision = source.source_revision(&workspace_manager);
    let runner = OxyMetricTreeRunner::new(workspace_manager, user.id, role)
        .with_scan(source.scan.clone())
        .with_engine_cache(engine_cache.cache.clone(), source_revision)
        .with_preagg(
            preagg_ctx.cache.clone(),
            preagg_ctx.renewal_threshold_secs_or(&config_manager),
        );
    let config = explain_config(req.config);

    let result = tokio::time::timeout(
        EXPLAIN_TIMEOUT,
        runner.run_explain(
            req.target,
            req.time_dimension,
            (req.current_period.0, req.current_period.1),
            (req.previous_period.0, req.previous_period.1),
            vec![],
            config,
        ),
    )
    .await
    .map_err(|_| {
        MetricTreeError::Op(format!(
            "explain timed out after {}s — consider narrowing the target measure or filtering dims",
            EXPLAIN_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| MetricTreeError::Op(e.to_string()))?;

    Ok(Json(result))
}

/// Resolve an [`OpportunityRequest`]'s optional instance into the engine scope.
///
/// No instance → an empty scope, i.e. size the whole population.
///
/// An instance that cannot be resolved is an error rather than an ignored
/// scope: silently sizing the population under a request that asked for one
/// instance is how a panel ends up reporting company-wide numbers under an
/// instance header.
fn opportunity_scope(
    layer: &oxy_airlayer_compat::SemanticLayer,
    req: &OpportunityRequest,
) -> Result<Vec<oxy_airlayer_compat::engine::query::QueryFilter>, MetricTreeError> {
    let Some(instance) = req.instance.as_ref() else {
        return Ok(Vec::new());
    };
    oxy_world_model::graph::instance_scope_filters(layer, &instance.entity, &instance.key)
        .ok_or_else(|| {
            MetricTreeError::NotFound(format!(
                "cannot scope '{}' to '{}': no entity named '{}' in the semantic model",
                req.target, instance.key, instance.entity
            ))
        })
}

/// `POST .../semantic/metric-tree/opportunity` — segment opportunity sizing.
pub async fn post_opportunity(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    preagg_ctx: PreaggCacheCtx,
    Json(req): Json<OpportunityRequest>,
) -> Result<Json<OpportunityResponse>, MetricTreeError> {
    // `source` owns the materialised tempdir; hold it until the layer below is
    // parsed. Nothing inside the blocking task reads it — the executor derives
    // its own (different) pre-aggregation cache root from the workspace manager.
    let source = resolve_scan(&workspace_manager).await?;
    let mut layer = load_layer_at(&source.scan)?;
    // Order is load-bearing. The tree is built from the clean layer: the
    // dispersion measure below is a pass-through, and pass-throughs read as
    // composite nodes, so a tree built after it would sprout an internal
    // `__opp_stddev__…` node into the world-model graph. The engine, by
    // contrast, must be built from the augmented layer — it resolves measure
    // names against its own copy, and would reject a measure it had never seen.
    let tree = oxy_semantic::build_metric_tree(&layer);
    oxy_airlayer_compat::engine::metric_tree_ops::augment_layer_for_opportunity(
        &mut layer,
        &req.target,
    );
    let scope = opportunity_scope(&layer, &req)?;
    // Resolved here because `layer` and `req` are both moved into the blocking
    // task below. Read from the augmented layer deliberately, but this is not
    // shadow-proof: `augment_layer_for_opportunity` also installs a
    // `MeasureType::Count` companion (`__opp_n__<measure>`) when the target
    // carries `.filters`, and `count_measure_id` — like the engine's own
    // `discover_count_measure` — takes the FIRST `Count` measure on the view.
    // On a view with a natural `count` measure, that one is declared first and
    // wins, so the synthetic companion never shows up here. On a view with NO
    // natural count measure, the synthetic companion becomes the only `Count`
    // found and gets reported as the denominator — naming a measure that
    // doesn't exist on the layer the analyst actually sees.
    let denominator = req
        .target
        .split_once('.')
        .and_then(|(view, _)| count_measure_id(&layer, view));
    let databases = workspace_databases(&workspace_manager);
    // Off the same `databases` the executor below queries, so the values its
    // downstream predict reads and the dialect it judges them by agree.
    let dialect =
        oxy_airlayer_compat::predict_dialect(&layer, &databases, &tree, &[req.target.as_str()]);
    // Not cached, and deliberately: `augment_layer_for_opportunity` above
    // rewrote `layer` for this request's target, so this engine is unique to
    // the request and shares nothing with the workspace's.
    let engine = std::sync::Arc::new(build_engine(layer.clone(), &databases)?);
    let handle = tokio::runtime::Handle::current();
    // Read here: the executor builds its connectors on a blocking thread.
    let holds_writes = crate::server::previews::request_hold::active();
    let preagg = crate::agentic_wiring::metric_tree_runner::RunnerPreagg {
        cache: preagg_ctx.cache.clone(),
        renewal_threshold_secs: preagg_ctx
            .renewal_threshold_secs_or(&workspace_manager.config_manager),
        // A read surface: an opportunity sizing is a display of the data, so a
        // rollup a cycle behind beats a warehouse scan.
        freshness: crate::server::preagg_context::RollupFreshness::ServeStale,
    };

    let result = tokio::task::spawn_blocking(move || {
        let executor = build_query_executor(
            engine,
            databases,
            workspace_manager,
            user.id,
            role,
            handle,
            preagg,
            holds_writes,
        );
        oxy_airlayer_compat::engine::metric_tree_ops::opportunity(
            &tree,
            &layer,
            &req.target,
            &req.time_dimension,
            (req.period.0.as_str(), req.period.1.as_str()),
            &scope,
            req.statistic,
            OPPORTUNITY_MIN_SUPPORT,
            dialect,
            &executor,
        )
    })
    .await
    .map_err(|e| MetricTreeError::Op(format!("opportunity task panicked: {e}")))?
    .map_err(|e| MetricTreeError::Op(e.to_string()))?;

    // Only "rows" mode divides by the count to form rates; reporting a
    // denominator for a scan that never used one would invite the reader to
    // read `current_value` as a rate when it is a raw measure value.
    let rate_denominator = (result.weight_basis == "rows")
        .then_some(denominator)
        .flatten();
    Ok(Json(OpportunityResponse {
        result,
        rate_denominator,
    }))
}

// ── Baseline (scenario simulation) ──────────────────────────────────────────

// The baseline budget (`BASELINE_TIMEOUT`, its headroom, the query budget and
// the shared deadline) lives beside the executor that spends it, in
// `metric_tree_baseline`; re-exported so every caller keeps its path.
pub(crate) use super::metric_tree_baseline::{BASELINE_TIMEOUT, baseline_query_deadline};

// The domain half — request/response shapes, the baseline reads and notes,
// the unvalued diff — lives in `oxy-metric-monitoring` (lowered with the
// projection); re-exported so every `server::api::metric_tree::` path holds.
pub use oxy_metric_monitoring::metric_tree_scenario::{
    BaselineInstance, BaselineReads, BaselineRequest, BaselineResponse, DistributionRequest,
    DrillRequest, DrillResponse, ExplainConfigOverride, ExplainRequest, OPPORTUNITY_MIN_SUPPORT,
    OpportunityInstance, OpportunityRequest, OpportunityResponse, PredictChange, PredictRequest,
    TimeDimensionsResponse, UnvaluedNode, UnvaluedReason, baseline_failure, baseline_note,
    baseline_reads, classify_unvalued, count_measure_id, derive_baseline_period, explain_config,
    predict_dialect, reject_lever_conflicts, target_supports_rate_basis, time_dimensions_of,
};

/// Resolve a [`BaselineRequest`]'s optional instance into the engine scope.
///
/// Mirrors [`opportunity_scope`]: no instance → an empty scope, i.e. value
/// the whole population. An instance that cannot be resolved is an error
/// rather than an ignored scope — silently valuing the whole population
/// under a request that named one instance is how a panel reports
/// company-wide numbers under an instance header.
fn baseline_scope(
    layer: &oxy_airlayer_compat::SemanticLayer,
    req: &BaselineRequest,
) -> Result<Vec<oxy_airlayer_compat::engine::query::QueryFilter>, MetricTreeError> {
    baseline_scope_core(layer, req.instance.as_ref()).map_err(MetricTreeError::NotFound)
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
pub(crate) fn baseline_scope_core(
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

/// Build the executor and run the baseline's per-view value and per-group fit
/// queries (see [`BASELINE_TIMEOUT`]), off the async runtime. Split out of
/// [`post_baseline`] to stay under the file's function-length budget —
/// mirrors the projects/ twin's `run_baseline_query`.
///
/// Unlike [`post_opportunity`], this does not go through
/// `OxyMetricTreeRunner` — the fan-out `baseline_reads` drives isn't part of
/// the `MetricTreeRunner` trait — so the executor is built directly here.
#[allow(clippy::too_many_arguments)]
async fn run_baseline_query(
    workspace_manager: WorkspaceManager<oxy::config::ReadOnly>,
    user_id: Uuid,
    role: WorkspaceRole,
    preagg_ctx: PreaggCacheCtx,
    layer: oxy_airlayer_compat::SemanticLayer,
    req: &BaselineRequest,
    scope: Vec<oxy_airlayer_compat::engine::query::QueryFilter>,
) -> Result<BaselineReads, MetricTreeError> {
    let tree = oxy_semantic::build_metric_tree(&layer);
    let databases = workspace_databases(&workspace_manager);
    let engine = std::sync::Arc::new(build_engine(layer.clone(), &databases)?);
    let handle = tokio::runtime::Handle::current();
    // Read here: the executor builds its connectors on a blocking thread.
    let holds_writes = crate::server::previews::request_hold::active();
    let preagg = crate::agentic_wiring::metric_tree_runner::RunnerPreagg {
        cache: preagg_ctx.cache.clone(),
        renewal_threshold_secs: preagg_ctx
            .renewal_threshold_secs_or(&workspace_manager.config_manager),
        // A read surface, same as `post_opportunity` above: a baseline is a
        // display of the data, so a rollup a cycle behind beats a warehouse
        // scan across every root in the batch.
        freshness: crate::server::preagg_context::RollupFreshness::ServeStale,
    };
    let roots = req.roots.clone();
    let period = req.period.clone();
    let time_dimension = req.time_dimension.clone();

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
            baseline_reads(
                &tree,
                &layer,
                &roots,
                &time_dimension,
                (period.0.as_str(), period.1.as_str()),
                &scope,
                executor,
            )
        }),
    )
    .await
    .map_err(|_| {
        MetricTreeError::Op(format!(
            "baseline timed out after {}s — consider narrowing the period or the scope",
            BASELINE_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| MetricTreeError::Op(format!("baseline task panicked: {e}")))
}

/// `POST .../semantic/metric-tree/baseline` — current values for the levers
/// and everything downstream of them.
pub async fn post_baseline(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    preagg_ctx: PreaggCacheCtx,
    Json(req): Json<BaselineRequest>,
) -> Result<Json<BaselineResponse>, MetricTreeError> {
    let source = resolve_scan(&workspace_manager).await?;
    let layer = load_layer_at(&source.scan)?;
    let tree = oxy_semantic::build_metric_tree(&layer);

    // Reject unknown levers up front: a typo must not read as "this measure
    // has no value", which is a completely different message in the UI.
    for root in &req.roots {
        if !tree.nodes.iter().any(|n| &n.id == root) {
            return Err(MetricTreeError::NotFound(format!(
                "measure '{root}' not in tree"
            )));
        }
    }

    let scope = baseline_scope(&layer, &req)?;

    let reads = run_baseline_query(
        workspace_manager,
        user.id,
        role,
        preagg_ctx,
        layer.clone(),
        &req,
        scope,
    )
    .await?;

    // Ask the engine WHY rather than inferring it from an empty map: an
    // executor error and an empty window are opposite problems with opposite
    // fixes, and guessing produced a message that told users the wrong one.
    let unvalued = classify_unvalued(
        &tree,
        &req.roots,
        &reads.values,
        &reads.outcome,
        &reads.skipped,
    );

    Ok(Json(BaselineResponse {
        values: reads.values,
        unvalued,
        resolved_period: req.period,
        baseline_note: baseline_note(&reads.outcome, &req.time_dimension, &reads.skipped),
        fitted: reads.fitted,
    }))
}

/// Resolve a [`DrillRequest`]'s optional instance into the engine scope.
///
/// Mirrors [`opportunity_scope`] against `DrillRequest`: no instance → an empty
/// scope (drill the whole population); an instance that cannot be resolved is an
/// error rather than a silently-ignored scope.
fn opportunity_scope_from_drill(
    layer: &oxy_airlayer_compat::SemanticLayer,
    req: &DrillRequest,
) -> Result<Vec<oxy_airlayer_compat::engine::query::QueryFilter>, MetricTreeError> {
    let Some(instance) = req.instance.as_ref() else {
        return Ok(Vec::new());
    };
    oxy_world_model::graph::instance_scope_filters(layer, &instance.entity, &instance.key)
        .ok_or_else(|| {
            MetricTreeError::NotFound(format!(
                "cannot scope '{}' to '{}': no entity named '{}' in the semantic model",
                req.target, instance.key, instance.entity
            ))
        })
}

/// `POST .../semantic/metric-tree/drill` — recursive gap decomposition.
///
/// Mirrors [`post_opportunity`] with three deltas: the augmented layer is shared
/// via `Arc<RwLock<>>`, the executor rebuilds its engine per query from that
/// shared layer (so the drill's mid-recursion synthetic measures compile), and
/// it calls `opportunity_drill` instead of `opportunity`.
pub async fn post_opportunity_drill(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    preagg_ctx: PreaggCacheCtx,
    Json(req): Json<DrillRequest>,
) -> Result<Json<DrillResponse>, MetricTreeError> {
    // `source` owns the materialised tempdir; hold it until the layer below is
    // parsed. Nothing inside the blocking task reads it — the executor derives
    // its own (different) pre-aggregation cache root from the workspace manager.
    let source = resolve_scan(&workspace_manager).await?;
    let mut clean_layer = load_layer_at(&source.scan)?;
    // Tree from the CLEAN layer (same reason as post_opportunity: augmentation
    // adds pass-through measures that would sprout graph nodes).
    let tree = oxy_semantic::build_metric_tree(&clean_layer);
    // Augment the ROOT target before sharing — the drill's children augment
    // themselves via dimension_candidates.
    oxy_airlayer_compat::engine::metric_tree_ops::augment_layer_for_opportunity(
        &mut clean_layer,
        &req.target,
    );
    // opportunity_scope + denominator read from the augmented layer, same as
    // post_opportunity.
    let scope = opportunity_scope_from_drill(&clean_layer, &req)?;
    let denominator = req
        .target
        .split_once('.')
        .and_then(|(view, _)| count_measure_id(&clean_layer, view));
    // Read the target's rate-basis eligibility before the layer is moved into
    // the Arc, so the response can gate the rate denominator on rows mode
    // (sum, or an eligible additive composite).
    let target_supports_rate_basis = target_supports_rate_basis(&clean_layer, &req.target);

    let databases = workspace_databases(&workspace_manager);
    let dialects = oxy_airlayer_compat::DatasourceDialectMap::from_config_databases(&databases);
    let shared: oxy_airlayer_compat::engine::metric_tree_ops::SharedLayer =
        std::sync::Arc::new(std::sync::RwLock::new(clean_layer));
    let handle = tokio::runtime::Handle::current();
    // Read here: the executor builds its connectors on a blocking thread.
    let holds_writes = crate::server::previews::request_hold::active();
    let preagg = crate::agentic_wiring::metric_tree_runner::RunnerPreagg {
        cache: preagg_ctx.cache.clone(),
        renewal_threshold_secs: preagg_ctx
            .renewal_threshold_secs_or(&workspace_manager.config_manager),
        // A read surface: an opportunity sizing is a display of the data, so a
        // rollup a cycle behind beats a warehouse scan.
        freshness: crate::server::preagg_context::RollupFreshness::ServeStale,
    };
    let default_config = oxy_airlayer_compat::engine::metric_tree_ops::DrillConfig::default();
    let config = oxy_airlayer_compat::engine::metric_tree_ops::DrillConfig {
        max_depth: req.max_depth.unwrap_or(default_config.max_depth),
        alpha: req.alpha.unwrap_or(default_config.alpha),
        root: req.root.clone(),
        ..default_config
    };

    let shared_for_exec = shared.clone();
    let result = tokio::task::spawn_blocking(move || {
        let executor = build_drill_query_executor(
            shared_for_exec,
            dialects,
            databases,
            workspace_manager,
            user.id,
            role,
            handle,
            preagg,
            holds_writes,
        );
        oxy_airlayer_compat::engine::metric_tree_ops::opportunity_drill(
            &tree,
            &shared,
            &req.target,
            &req.time_dimension,
            (req.period.0.as_str(), req.period.1.as_str()),
            &scope,
            &config,
            req.statistic,
            OPPORTUNITY_MIN_SUPPORT,
            &executor,
        )
    })
    .await
    .map_err(|e| MetricTreeError::Op(format!("drill task panicked: {e}")))?
    .map_err(|e| MetricTreeError::Op(e.to_string()))?;

    // A rate denominator only makes sense when the drill actually formed per-unit
    // rates — i.e. rows mode, which airlayer gates to a `supports_rate_basis`
    // target (a sum, or an eligible additive composite). `opportunity_drill`
    // returns `Some` for any additive root (its dimensions are populated in
    // value_share/equal mode too), so gating on `Some` alone would mislabel a raw
    // value as a rate. Require BOTH a result and a rate-basis target, mirroring
    // `post_opportunity`'s `weight_basis == "rows"` gate.
    let rate_denominator = result
        .as_ref()
        .filter(|_| target_supports_rate_basis)
        .and(denominator);
    Ok(Json(DrillResponse {
        result,
        rate_denominator,
    }))
}

/// `GET .../semantic/metric-tree/time-dimensions` — list valid time
/// dimensions per view. Lets clients (the metric-tree UI in particular)
/// drop hardcoded curated maps and discover what's actually queryable.
pub async fn get_time_dimensions(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
) -> Result<Json<TimeDimensionsResponse>, MetricTreeError> {
    let source = resolve_scan(&workspace_manager).await?;
    let layer = load_layer_at(&source.scan)?;
    Ok(Json(time_dimensions_of(&layer)))
}

/// `POST .../semantic/metric-tree/distribution` — distribution view of a
/// measure. Returns an `ExplainResult`-shaped payload (so the same client
/// renderers work), but with the baseline auto-derived as the equal-length
/// window immediately before `period`. Callers only specify the period of
/// interest; ignore the delta fields when rendering a pure distribution.
pub async fn post_distribution(
    WorkspaceManagerReadOnly(workspace_manager): WorkspaceManagerReadOnly,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    EffectiveWorkspaceRole(role): EffectiveWorkspaceRole,
    preagg_ctx: PreaggCacheCtx,
    engine_cache: SemanticEngineCacheCtx,
    Json(req): Json<DistributionRequest>,
) -> Result<Json<ExplainResult>, MetricTreeError> {
    use agentic_analytics::MetricTreeRunner as _;

    let baseline = derive_baseline_period(&req.period.0, &req.period.1)
        .ok_or_else(|| MetricTreeError::Op("invalid period dates (expected YYYY-MM-DD)".into()))?;

    // `source` owns the materialised tempdir; keep it alive for the whole run.
    let source = resolve_scan(&workspace_manager).await?;
    let config_manager = workspace_manager.config_manager.clone();
    // Resolved before the manager moves into the runner. What `source` actually
    // READ, not the request's pinned revision — those differ on a node that
    // holds a working copy and is pinned to a revision.
    let source_revision = source.source_revision(&workspace_manager);
    let runner = OxyMetricTreeRunner::new(workspace_manager, user.id, role)
        .with_scan(source.scan.clone())
        .with_engine_cache(engine_cache.cache.clone(), source_revision)
        .with_preagg(
            preagg_ctx.cache.clone(),
            preagg_ctx.renewal_threshold_secs_or(&config_manager),
        );
    let config = ExplainConfig::default();

    let result = tokio::time::timeout(
        EXPLAIN_TIMEOUT,
        runner.run_explain(
            req.target,
            req.time_dimension,
            (req.period.0.clone(), req.period.1.clone()),
            baseline,
            vec![],
            config,
        ),
    )
    .await
    .map_err(|_| {
        MetricTreeError::Op(format!(
            "distribution timed out after {}s — try narrowing the target measure",
            EXPLAIN_TIMEOUT.as_secs()
        ))
    })?
    .map_err(|e| MetricTreeError::Op(e.to_string()))?;

    Ok(Json(result))
}

#[cfg(test)]
mod tests {

    /// The handler source, excluding this test module — which necessarily
    /// names the very symbols the tests below assert are absent.
    fn handler_src() -> &'static str {
        include_str!("metric_tree.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("source splits on its own test-module attribute")
    }

    /// The handlers must resolve their scan root through the compile boundary,
    /// never from the workspace working copy — a stateless serve replica has
    /// none, and reading it there 500'd every metric-tree call on every
    /// workspace (oxy-hq/oxygen#878). `load_layer_sync` is gone, but
    /// `config_manager.semantics_scan_path()` is still one autocomplete away,
    /// and it compiles fine — it just fails in cloud. So guard the shape, in
    /// the spirit of `tests/authz/authz_boundaries.rs`.
    #[test]
    fn handlers_never_scan_the_working_copy() {
        assert!(
            !handler_src().contains("semantics_scan_path"),
            "metric_tree.rs must resolve its scan root via resolve_query_scan_source \
             (compile boundary first), not config_manager.semantics_scan_path()"
        );
    }

    #[test]
    fn both_baseline_surfaces_take_the_budget_from_one_place() {
        // `metric_tree_projection` carried its own `BASELINE_TIMEOUT / 2`. Two
        // handlers hitting the same warehouse with two silently diverging
        // deadlines is exactly what `BASELINE_TIMEOUT` was made `pub(crate)` to
        // prevent, and the fraction escaped that rule by being written out
        // longhand at each call site.
        //
        // Comment lines are skipped, or this gate matches the very prose above
        // that explains why the fraction is gone — the same trap
        // `authz::shared_db_registry` documents. A gate that counts prose is a
        // gate that fires for the wrong reason.
        for (name, src) in [
            ("metric_tree.rs", include_str!("metric_tree.rs")),
            (
                "metric_tree_projection.rs",
                include_str!("metric_tree_projection.rs"),
            ),
            // The domain halves spend the same budget (`baseline_reads`, the
            // projection's per-view read), so the guard follows them down.
            (
                "metric-monitoring/src/metric_tree_scenario.rs",
                include_str!("../../../../metric-monitoring/src/metric_tree_scenario.rs"),
            ),
            (
                "metric-monitoring/src/metric_tree_projection.rs",
                include_str!("../../../../metric-monitoring/src/metric_tree_projection.rs"),
            ),
        ] {
            let offender = src
                .split("\n#[cfg(test)]")
                .next()
                .expect("source splits on its own test-module attribute")
                .lines()
                .find(|l| !l.trim_start().starts_with("//") && l.contains("BASELINE_TIMEOUT /"));
            assert_eq!(
                offender, None,
                "{name} re-derives the baseline query budget as a fraction of \
                 BASELINE_TIMEOUT at the call site. It is BASELINE_QUERY_BUDGET, \
                 reached through baseline_query_deadline(), so the two baseline \
                 surfaces cannot be tuned apart."
            );
        }
    }
}
