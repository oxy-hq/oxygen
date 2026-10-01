//! Time-series projection for the Metric Tree scenario canvas.
//!
//! The scenario's [`baseline`] answers "what is this measure worth over the
//! window"; this answers "what has it been doing, and what does it do next".
//! One batched bucketed query, then [`oxy_metric_monitoring::project`] — the
//! detector's own MSTL + AutoETS model, pointed forward — per measure.
//!
//! **What this endpoint deliberately does not return is the scenario curve.**
//! It returns the *baseline* curve only. Composing the second curve is pure
//! arithmetic over the projection and the `predict` result — a uniform
//! proportional shift, landing `lag` buckets in — and it belongs on the client
//! for exactly the reason propagation does: the analyst edits a lever value on
//! every keystroke, and a warehouse query behind each character is the thing
//! the baseline/predict split exists to prevent. So the split is now three
//! deep: `baseline` (levels, one query), `projection` (curves, one query),
//! `predict` (pure, per keystroke) — and only the first two touch a database.
//!
//! [`baseline`]: super::metric_tree::post_baseline

use std::collections::HashMap;

use axum::extract::Json;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy_airlayer_compat::engine::metric_tree::MetricTree;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_metric_monitoring::MonitorEntry;
use oxy_metric_monitoring::config::Granularity;
use oxy_metric_monitoring::forecast::{
    DEFAULT_INTERVAL_LEVEL, ProjectInputs, ProjectedBucket, project,
};

use crate::agentic_wiring::metric_tree_runner::{
    build_query_executor, build_time_series_query_request, parse_flexible_date,
};
use crate::server::api::metric_tree::{
    BASELINE_TIMEOUT, BaselineInstance, MetricTreeError, baseline_query_deadline,
    baseline_scope_core, build_engine, load_layer_at, resolve_scan, workspace_databases,
};
use crate::server::api::metric_tree_baseline::{
    BUDGET_SPENT, GroupFailure, PartialSplitExecutor, forward_reachable,
};
use crate::server::api::middlewares::workspace_context::{
    EffectiveWorkspaceRole, PreaggCacheCtx, WorkspaceManagerReadOnly,
};

/// Buckets a caller may ask for past the last historical one.
///
/// A refusal rather than a clamp. Silently truncating a horizon of 5,000 to
/// 365 would return a curve that stops early with nothing saying why, and the
/// analyst reads a forecast that ends in March as a forecast that ends in
/// March. Refusing names the number they have to change.
pub(crate) const MAX_PROJECTION_HORIZON: usize = 365;

#[derive(Debug, Deserialize)]
pub struct ProjectionRequest {
    /// Lever node ids. Curves are drawn for these plus everything
    /// forward-reachable from them — the same set the baseline values.
    pub roots: Vec<String>,
    pub time_dimension: String,
    /// `[start, end]` inclusive date strings for the HISTORY. Deliberately its
    /// own window rather than the baseline's: a fit needs eight seasonal
    /// cycles, which is far more history than a scenario baseline usually
    /// wants to average over.
    pub period: (String, String),
    /// Narrow to one world-model instance. `None` projects the whole
    /// population. Same shape as the baseline's, because it is the same picker.
    pub instance: Option<BaselineInstance>,
    #[serde(default = "default_granularity")]
    pub granularity: Granularity,
    /// Buckets to project past the last historical one.
    pub horizon: usize,
    /// Seasonal periods, in buckets, for **every** measure in this request.
    ///
    /// Absent is the normal case and does not mean "the default" — it means
    /// *resolve per measure*, from whatever monitor already watches that series
    /// (see [`resolve_seasonality`]). Supplied, it pins the decomposition and
    /// the monitors are not consulted, which is the only way a caller can ask
    /// for a cycle nobody has declared yet.
    #[serde(default)]
    pub seasonality: Option<Vec<usize>>,
}

fn default_granularity() -> Granularity {
    Granularity::Day
}

#[derive(Debug, Serialize)]
pub struct ProjectionResponse {
    pub granularity: String,
    /// Echoed back: the query is expensive and the client caches on it.
    pub resolved_period: (String, String),
    pub horizon: usize,
    pub series: Vec<MeasureProjection>,
    /// Why the whole projection is empty, in words the UI can show. `None`
    /// when at least one measure produced history.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection_note: Option<String>,
}

/// One measure's curve: what happened, then what the model expects next.
///
/// `forecast` empty with a `refusal` set is the honest state, and it is not
/// the same as a flat forward line — which is what any layer that defaults the
/// missing curve to "unchanged" would draw. There is no third field meaning
/// "no forecast but no reason": a measure that was asked for always comes back
/// saying what happened to it.
#[derive(Debug, Serialize, PartialEq)]
pub struct MeasureProjection {
    pub measure: String,
    pub history: Vec<HistoryPoint>,
    pub forecast: Vec<ForecastPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
    /// The seasonal periods this curve was decomposed against.
    ///
    /// On the wire because it is resolved per measure and is otherwise
    /// invisible: two curves in the same response can legitimately assume
    /// different cycles, and a band whose seasonal model is implicit is a band
    /// nobody can reconcile against the monitor that scores the same series.
    pub seasonality: Vec<usize>,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct HistoryPoint {
    /// Bucket start, `YYYY-MM-DD`.
    pub date: String,
    pub value: f64,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct ForecastPoint {
    pub date: String,
    pub point: f64,
    /// The prediction interval, `null` when the model returned none.
    ///
    /// `Option`, not a bare `f64`: an absent bound is ±infinity, which
    /// `serde_json` writes as `null` anyway — but going through the option
    /// makes that a decision rather than a coincidence, and stops a future
    /// `unwrap_or(point)` from collapsing "unknown spread" onto the point and
    /// drawing a band of zero width where there is no band at all.
    pub lower: Option<f64>,
    pub upper: Option<f64>,
}

/// A finite bound, or `None`. Infinities and NaN are not numbers a chart can
/// draw and are not claims the model is making.
fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

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
    let layer = load_layer_at(&source.scan_path)?;
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

/// Everything checkable before a query is worth paying for.
///
/// An unknown lever is a 404 for the same reason it is on the baseline: a typo
/// must read as *unknown measure*, never as *this measure has no history*,
/// which is a different message with a different fix.
fn validate(tree: &MetricTree, req: &ProjectionRequest) -> Result<(), MetricTreeError> {
    for root in &req.roots {
        if !tree.nodes.iter().any(|n| &n.id == root) {
            return Err(MetricTreeError::NotFound(format!(
                "measure '{root}' not in tree"
            )));
        }
    }
    if req.horizon == 0 || req.horizon > MAX_PROJECTION_HORIZON {
        return Err(MetricTreeError::BadRequest(format!(
            "horizon must be between 1 and {MAX_PROJECTION_HORIZON} buckets, got {}",
            req.horizon
        )));
    }
    validate_seasonality(req)?;
    Ok(())
}

/// A supplied `seasonality` must be a list of real cycles.
///
/// Both refusals name something the caller can only have meant by mistake. An
/// empty list is not "use the default" — omitting the field is; sending `[]`
/// asks for a decomposition against no cycle at all. A period of 1 repeats
/// every bucket and a period of 0 is not a period, and MSTL's own error for
/// either arrives as a fit failure attributed to the *series*, which sends the
/// reader looking at their data for a fault that is in their request.
fn validate_seasonality(req: &ProjectionRequest) -> Result<(), MetricTreeError> {
    let Some(periods) = req.seasonality.as_ref() else {
        return Ok(());
    };
    if periods.is_empty() {
        return Err(MetricTreeError::BadRequest(
            "seasonality must name at least one period — omit the field to resolve it from the \
             measure's monitor, or from the granularity default"
                .to_string(),
        ));
    }
    if let Some(bad) = periods.iter().find(|p| **p < 2) {
        return Err(MetricTreeError::BadRequest(format!(
            "seasonality periods count buckets and must each be >= 2, got {bad}"
        )));
    }
    Ok(())
}

/// The seasonal periods this measure's curve will be decomposed against.
///
/// An explicit request wins outright — it is the only way to ask for a cycle
/// nobody has declared. Otherwise the periods come from whatever monitor
/// already watches this exact series, which is what keeps the band on the
/// canvas the band an anomaly had to breach.
fn seasonality_for(
    req: &ProjectionRequest,
    monitors: &[MonitorEntry],
    measure: &str,
) -> Vec<usize> {
    match req.seasonality.as_ref() {
        Some(periods) => periods.clone(),
        None => resolve_seasonality(monitors, measure, &req.time_dimension, req.granularity),
    }
}

/// The seasonal periods a monitor already scores this series against, or the
/// granularity default when none does.
///
/// "This series" is the full triple — measure, time dimension and grain — not
/// the measure alone. A monthly monitor on the same measure is decomposing
/// different buckets, and lending its `[12]` to a daily projection would claim
/// a twelve-*day* cycle nobody declared.
///
/// Entries are compared on [`MonitorEntry::effective_seasonality`], so a
/// segment monitor that declares nothing folds into the same default and reads
/// as agreement rather than as a conflict. When the matches genuinely disagree
/// there is no single band to line up with, and picking one would make the
/// canvas match whichever entry happened to be written first — so the default
/// is used and neither monitor is silently privileged.
fn resolve_seasonality(
    monitors: &[MonitorEntry],
    measure: &str,
    time_dimension: &str,
    granularity: Granularity,
) -> Vec<usize> {
    let mut matched = monitors
        .iter()
        .filter(|m| {
            m.measure == measure
                && m.time_dimension == time_dimension
                && m.granularity == granularity
        })
        .map(MonitorEntry::effective_seasonality);

    let Some(first) = matched.next() else {
        return granularity.default_seasonality();
    };
    if matched.all(|other| other == first) {
        first
    } else {
        granularity.default_seasonality()
    }
}

/// What a projection takes from the workspace's `.monitor.yml`.
///
/// Both fields come out of ONE deserialised [`oxy_metric_monitoring::MonitorConfig`],
/// which is the point: the timezone used to be a separate raw read of the
/// working copy, so a serve replica without one bucketed in UTC while an IDE
/// node bucketed in the declared zone — and the customer-app twin cached
/// whichever answer filled it first.
#[derive(Debug, Default)]
struct MonitorSettings {
    /// File-level `timezone`; `None` buckets in UTC.
    timezone: Option<String>,
    monitors: Vec<MonitorEntry>,
}

/// The pure half of [`load_monitor_settings`]: a compiled `.monitor.yml`
/// definition to the two things a projection reads from it.
fn parse_monitor_settings(definition: serde_json::Value) -> serde_json::Result<MonitorSettings> {
    let cfg: oxy_metric_monitoring::MonitorConfig = serde_json::from_value(definition)?;
    Ok(MonitorSettings {
        timezone: cfg.timezone,
        monitors: cfg.monitors,
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

/// Why nothing came back at all, when nothing did.
///
/// The two ways to get here are opposite problems with opposite fixes — the
/// warehouse rejected the query, versus the window is genuinely empty — so the
/// executor's own error is carried down rather than inferred from the empty
/// map. Folding them together is what makes a surface tell users to lengthen a
/// window that was never the problem.
///
/// Only the whole-query case is stated here. A single measure's silence is its
/// own `refusal`, which names something specific to that series.
/// Only fires when EVERY measure failed or came back empty. A partial failure
/// is each measure's own `refusal`, because a banner over a panel that is
/// drawing three working curves is a banner that reads as "none of this is
/// real".
fn projection_note(
    history: &SeriesByMeasure,
    failures: &HashMap<String, String>,
    measures: &[String],
) -> Option<String> {
    if !measures.is_empty() && measures.iter().all(|m| failures.contains_key(m)) {
        // Every distinct message, so a split that failed differently per group
        // does not report only whichever one hashed first.
        let mut reasons: Vec<&str> = failures.values().map(String::as_str).collect();
        reasons.sort_unstable();
        reasons.dedup();
        // Same reason `project_one` is neutral here: a budget that ran out is
        // not a rejection, and every reason already names its own cause.
        return Some(format!("no series could be read: {}", reasons.join("; ")));
    }
    if !failures.is_empty() {
        return None;
    }
    history.values().all(Vec::is_empty).then(|| {
        "no rows in this period on this time dimension — try a longer window, or a time \
         dimension the pinned measure actually has data for"
            .to_string()
    })
}

/// Fit and project one measure, or say why not.
///
/// A query failure for THIS measure short-circuits: it has no history, and
/// "not enough history to fit" would be a true sentence pointing at the wrong
/// problem — the series was never fetched.
fn project_one(
    measure: &str,
    history: &[(NaiveDate, f64)],
    query_failure: Option<&str>,
    granularity: Granularity,
    horizon: usize,
    seasonality: Vec<usize>,
) -> MeasureProjection {
    if let Some(reason) = query_failure {
        return MeasureProjection {
            measure: measure.to_string(),
            history: Vec::new(),
            forecast: Vec::new(),
            // Neutral about the cause, because the reason is not always a
            // rejection: a spent query budget means nothing was ever asked, and
            // "the warehouse rejected this" would point triage at a warehouse
            // that never saw the query. The reason names its own cause.
            refusal: Some(format!("this measure could not be read: {reason}")),
            seasonality,
        };
    }
    let points = history
        .iter()
        .map(|(date, value)| HistoryPoint {
            date: date.to_string(),
            value: *value,
        })
        .collect();
    let projected = project(ProjectInputs {
        history,
        granularity,
        seasonal_periods: seasonality.clone(),
        horizon,
        interval_level: DEFAULT_INTERVAL_LEVEL,
    });
    match projected {
        Ok(buckets) => MeasureProjection {
            measure: measure.to_string(),
            history: points,
            forecast: buckets.iter().map(to_wire).collect(),
            refusal: None,
            seasonality,
        },
        // A refusal is reported, never implied. Without it a measure with too
        // little history is indistinguishable from one the UI failed to draw.
        Err(e) => MeasureProjection {
            measure: measure.to_string(),
            history: points,
            forecast: Vec::new(),
            refusal: Some(e.to_string()),
            seasonality,
        },
    }
}

fn to_wire(bucket: &ProjectedBucket) -> ForecastPoint {
    ForecastPoint {
        date: bucket.date.to_string(),
        point: bucket.point,
        lower: finite(bucket.lower),
        upper: finite(bucket.upper),
    }
}

/// Bucketed history per measure, ascending by date.
type SeriesByMeasure = HashMap<String, Vec<(NaiveDate, f64)>>;

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
    let holds_writes = crate::server::previews::request_hold::active();
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

/// Run the batched request; on a refusal, retry one request per source view.
///
/// The whole batch failing because of one measure is the failure mode this
/// exists to break up. Airlayer picks its SQL shape per MEASURE — a single
/// non-additive measure on a multiplied view routes the entire request down a
/// path with its own constraints — so a tree with one awkward measure took
/// every other curve down with it.
///
/// Grouped by **view** rather than per measure: measures from one view already
/// share a CTE, so a view is the narrowest group that cannot itself have
/// created the mixture, and the [`PartialSplitExecutor`] splits it further by
/// additivity if that is what refused. Per-measure would spend a round trip
/// per node for no additional narrowing.
///
/// Deliberately NOT all-or-nothing, which is where this differs from
/// `metric_tree_baseline::run_with_split`. There, a partially-answered batch is
/// dangerous: a measure that was never successfully asked for is
/// indistinguishable from one the warehouse returned no rows for, and the
/// baseline's `unvalued` diff would file it under the wrong reason. Here every
/// measure carries its OWN refusal string, so a partial answer stays fully
/// attributed.
///
/// Which is why the executor is a [`PartialSplitExecutor`] and not a bare
/// `QueryExecutor`. The retries below re-enter the split, and the split used to
/// throw away the groups it HAD answered on any sibling's failure — so this
/// function's argument for a partial answer stopped applying one layer down,
/// and a view whose additive half was fine still lost every curve in it.
///
/// The loop also stops at the executor's deadline. The split checks the same
/// clock between its groups; without this check the retries here would go on
/// issuing the initial query of each group after the budget the split is
/// honouring had already been spent.
fn read_series(
    executor: &PartialSplitExecutor,
    request: &oxy_airlayer_compat::engine::query::QueryRequest,
    measures: &[String],
    dim_alias: &str,
    trim: Option<(NaiveDate, NaiveDate)>,
) -> (SeriesByMeasure, HashMap<String, String>) {
    let batched = match executor.run(request) {
        Ok((rows, failed)) => return attributed(&rows, measures, &failed, dim_alias, trim),
        Err(e) => e,
    };
    tracing::warn!(
        error = %batched,
        measures = measures.len(),
        "metric-tree projection batch failed; retrying per source view"
    );

    let mut series = SeriesByMeasure::new();
    let mut failures: HashMap<String, String> = HashMap::new();
    for group in group_by_view(measures) {
        if executor.out_of_budget() {
            // Not "the warehouse rejected it": this query was never issued.
            // The distinction is the whole point of naming the budget — one
            // sends an analyst to their warehouse, the other to a shorter
            // period.
            tracing::warn!(
                group = ?group,
                "metric-tree projection ran out of query budget before this view's retry"
            );
            for measure in group {
                failures.insert(measure, BUDGET_SPENT.to_string());
            }
            continue;
        }
        // Clone rather than rebuild: the filters, the time dimension and the
        // timezone pad are already resolved on the batched request, and
        // recomputing them here would be a second place for the window to
        // drift from the one the caller asked for.
        let mut scoped = request.clone();
        scoped.measures.clone_from(&group);
        match executor.run(&scoped) {
            Ok((rows, failed)) => {
                let (got, lost) = attributed(&rows, &group, &failed, dim_alias, trim);
                series.extend(got);
                failures.extend(lost);
            }
            Err(e) => {
                let reason = e.to_string();
                for measure in group {
                    failures.insert(measure, reason.clone());
                }
            }
        }
    }
    (series, failures)
}

/// Split one response into the series it carries and the measures it lost.
///
/// A measure whose group failed must get NO entry in the series map:
/// [`parse_series`] seeds every measure it is handed with an empty vector, and
/// an empty series with no refusal beside it is the "unquantifiable rendered as
/// 0" this surface exists to prevent. It gets its group's reason instead.
fn attributed(
    rows: &[serde_json::Map<String, serde_json::Value>],
    asked: &[String],
    failed: &[GroupFailure],
    dim_alias: &str,
    trim: Option<(NaiveDate, NaiveDate)>,
) -> (SeriesByMeasure, HashMap<String, String>) {
    let mut failures: HashMap<String, String> = HashMap::new();
    for group in failed {
        for measure in &group.measures {
            failures.insert(measure.clone(), group.reason.clone());
        }
    }
    let answered: Vec<String> = asked
        .iter()
        .filter(|m| !failures.contains_key(*m))
        .cloned()
        .collect();
    (parse_series(rows, &answered, dim_alias, trim), failures)
}

/// Measures grouped by their source view, in first-seen order so the retries
/// are deterministic and an executor cache in front of them can hit.
fn group_by_view(measures: &[String]) -> Vec<Vec<String>> {
    let mut order: Vec<&str> = Vec::new();
    let mut groups: HashMap<&str, Vec<String>> = HashMap::new();
    for measure in measures {
        let view = measure.split('.').next().unwrap_or(measure.as_str());
        if !groups.contains_key(view) {
            order.push(view);
        }
        groups.entry(view).or_default().push(measure.clone());
    }
    order
        .into_iter()
        .filter_map(|view| groups.remove(view))
        .collect()
}

/// Split the bucketed rows into one ascending series per measure.
///
/// A row missing a measure's column contributes nothing to that measure rather
/// than a zero — the same rule the baseline's `unvalued` diff encodes. An
/// invented zero here would reach the forecaster as a measured collapse and
/// bend the trend through it.
pub(crate) fn parse_series(
    rows: &[serde_json::Map<String, serde_json::Value>],
    measures: &[String],
    dim_alias: &str,
    trim: Option<(NaiveDate, NaiveDate)>,
) -> SeriesByMeasure {
    let aliases: Vec<(String, String)> = measures
        .iter()
        .map(|m| (m.clone(), m.replace('.', "__")))
        .collect();
    let mut out: SeriesByMeasure = measures.iter().map(|m| (m.clone(), Vec::new())).collect();

    for row in rows {
        let Some(date) = row
            .get(dim_alias)
            .and_then(|v| v.as_str().map(str::to_string))
            .or_else(|| row.get(dim_alias).map(|v| v.to_string()))
            .and_then(|s| parse_flexible_date(s.trim_matches('"')))
        else {
            continue;
        };
        if let Some((start, end)) = trim {
            if date < start || date > end {
                continue;
            }
        }
        for (measure, alias) in &aliases {
            if let Some(value) = row.get(alias).and_then(serde_json::Value::as_f64) {
                out.entry(measure.clone()).or_default().push((date, value));
            }
        }
    }

    // The query orders by the bucket, but a split retry merges two responses,
    // so sort rather than trust arrival order — MSTL reads the array as
    // uniformly spaced and a single out-of-order bucket misaligns every
    // seasonal index after it.
    for series in out.values_mut() {
        series.sort_by_key(|(date, _)| *date);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_metric_monitoring::MonitorEntry;
    use serde_json::json;

    fn entry(measure: &str, seasonality: Option<Vec<usize>>) -> MonitorEntry {
        serde_json::from_value(json!({
            "measure": measure,
            "time_dimension": "store_days.business_date",
            "granularity": "day",
            "seasonality": seasonality,
        }))
        .expect("a monitor entry needs only a measure and a time dimension")
    }

    /// The invariant the whole resolution exists for: a monitor that declares
    /// its own seasonal periods is scored against them, so the curve drawn for
    /// that series has to be decomposed against them too. Otherwise one measure
    /// carries two bands and only one is the one an anomaly must breach.
    #[test]
    fn honours_a_seasonality_the_monitor_declares() {
        let monitors = vec![entry("store_days.net_sales", Some(vec![7, 365]))];
        assert_eq!(
            resolve_seasonality(
                &monitors,
                "store_days.net_sales",
                "store_days.business_date",
                Granularity::Day,
            ),
            vec![7, 365]
        );
    }

    #[test]
    fn falls_back_to_the_granularity_default_when_no_monitor_watches_the_measure() {
        let monitors = vec![entry("store_days.labor_cost", Some(vec![7, 365]))];
        assert_eq!(
            resolve_seasonality(
                &monitors,
                "store_days.net_sales",
                "store_days.business_date",
                Granularity::Day,
            ),
            Granularity::Day.default_seasonality()
        );
    }

    /// A monitor on the same measure at a different grain describes a different
    /// series. Borrowing its periods would apply `[12]` monthly cycles to daily
    /// buckets.
    #[test]
    fn a_monitor_at_another_granularity_is_not_the_same_series() {
        let mut monthly = entry("store_days.net_sales", Some(vec![12]));
        monthly.granularity = Granularity::Month;
        assert_eq!(
            resolve_seasonality(
                &[monthly],
                "store_days.net_sales",
                "store_days.business_date",
                Granularity::Day,
            ),
            Granularity::Day.default_seasonality()
        );
    }

    /// Same measure, same grain, a different time dimension — also a different
    /// series, and its cycle is not transferable.
    #[test]
    fn a_monitor_on_another_time_dimension_is_not_the_same_series() {
        let mut other = entry("store_days.net_sales", Some(vec![7, 365]));
        other.time_dimension = "store_days.closed_at".to_string();
        assert_eq!(
            resolve_seasonality(
                &[other],
                "store_days.net_sales",
                "store_days.business_date",
                Granularity::Day,
            ),
            Granularity::Day.default_seasonality()
        );
    }

    /// Segment monitors that declare nothing resolve to the same default, so
    /// they agree and must not be read as a conflict.
    #[test]
    fn segment_monitors_that_agree_still_resolve() {
        let monitors = vec![
            entry("store_days.net_sales", Some(vec![7, 365])),
            entry("store_days.net_sales", Some(vec![7, 365])),
        ];
        assert_eq!(
            resolve_seasonality(
                &monitors,
                "store_days.net_sales",
                "store_days.business_date",
                Granularity::Day,
            ),
            vec![7, 365]
        );
    }

    /// Two monitors on one series claiming different cycles leave no single
    /// band to agree with. Picking one would make the canvas match whichever
    /// entry happened to be written first — so neither is picked.
    #[test]
    fn conflicting_monitors_fall_back_rather_than_pick_one() {
        let monitors = vec![
            entry("store_days.net_sales", Some(vec![7, 365])),
            entry("store_days.net_sales", Some(vec![7, 30])),
        ];
        assert_eq!(
            resolve_seasonality(
                &monitors,
                "store_days.net_sales",
                "store_days.business_date",
                Granularity::Day,
            ),
            Granularity::Day.default_seasonality()
        );
    }

    #[test]
    fn an_explicit_request_seasonality_outranks_every_monitor() {
        let monitors = vec![entry("store_days.net_sales", Some(vec![7, 365]))];
        let mut req = projection_request();
        req.seasonality = Some(vec![7, 30]);
        assert_eq!(
            seasonality_for(&req, &monitors, "store_days.net_sales"),
            vec![7, 30]
        );
    }

    #[test]
    fn without_an_override_the_request_resolves_per_measure() {
        let monitors = vec![entry("store_days.net_sales", Some(vec![7, 365]))];
        let req = projection_request();
        assert_eq!(
            seasonality_for(&req, &monitors, "store_days.net_sales"),
            vec![7, 365]
        );
        assert_eq!(
            seasonality_for(&req, &monitors, "store_days.labor_cost"),
            Granularity::Day.default_seasonality()
        );
    }

    /// The timezone and the entries come out of the same compiled definition,
    /// so a replica that never held a working copy buckets in the declared
    /// zone exactly as the IDE node does.
    #[test]
    fn monitor_settings_carry_the_file_level_timezone_and_the_entries() {
        let settings = parse_monitor_settings(json!({
            "timezone": "America/Los_Angeles",
            "monitors": [{
                "measure": "store_days.net_sales",
                "time_dimension": "store_days.business_date",
                "granularity": "day",
                "seasonality": [7, 365],
            }],
        }))
        .expect("a well-formed monitor config deserialises");
        assert_eq!(settings.timezone.as_deref(), Some("America/Los_Angeles"));
        assert_eq!(settings.monitors.len(), 1);
        assert_eq!(settings.monitors[0].effective_seasonality(), vec![7, 365]);
    }

    /// No `timezone:` is UTC, and it is still a valid file with entries.
    #[test]
    fn a_monitor_config_without_a_timezone_buckets_in_utc() {
        let settings = parse_monitor_settings(json!({
            "monitors": [{
                "measure": "store_days.net_sales",
                "time_dimension": "store_days.business_date",
            }],
        }))
        .expect("timezone is optional");
        assert_eq!(settings.timezone, None);
        assert_eq!(settings.monitors.len(), 1);
    }

    /// A malformed file is the scan path's error to report; here it costs the
    /// defaults, never the projection.
    #[test]
    fn a_malformed_monitor_config_is_an_error_the_loader_defaults() {
        assert!(parse_monitor_settings(json!({ "monitors": "not a list" })).is_err());
    }

    fn projection_request() -> ProjectionRequest {
        ProjectionRequest {
            roots: vec!["store_days.net_sales".to_string()],
            time_dimension: "store_days.business_date".to_string(),
            period: ("2025-07-20".to_string(), "2026-07-19".to_string()),
            instance: None,
            granularity: Granularity::Day,
            horizon: 90,
            seasonality: None,
        }
    }

    /// A period of 1 repeats every bucket and a period of 0 is not a cycle;
    /// both reach MSTL as a decomposition it cannot perform. Refused at the
    /// boundary, where the message can name the field.
    #[test]
    fn rejects_a_seasonal_period_below_two() {
        let mut req = projection_request();
        req.seasonality = Some(vec![7, 1]);
        let err = validate_seasonality(&req).expect_err("period 1 is not a cycle");
        assert!(
            matches!(&err, MetricTreeError::BadRequest(m) if m.contains("seasonality")),
            "expected a 400 naming the field, got {err:?}"
        );
    }

    #[test]
    fn rejects_an_empty_seasonality_list() {
        let mut req = projection_request();
        req.seasonality = Some(Vec::new());
        assert!(
            validate_seasonality(&req).is_err(),
            "an empty list is not 'use the default' — that is what omitting the field means"
        );
    }

    #[test]
    fn accepts_an_absent_seasonality() {
        assert!(validate_seasonality(&projection_request()).is_ok());
    }

    fn row(date: &str, pairs: &[(&str, f64)]) -> serde_json::Map<String, serde_json::Value> {
        let mut map = serde_json::Map::new();
        map.insert("v__d__day".to_string(), json!(date));
        for (alias, value) in pairs {
            map.insert((*alias).to_string(), json!(value));
        }
        map
    }

    fn measures() -> Vec<String> {
        vec!["v.a".to_string(), "v.b".to_string()]
    }

    #[test]
    fn splits_rows_into_one_series_per_measure() {
        let rows = vec![
            row("2026-01-01", &[("v__a", 1.0), ("v__b", 10.0)]),
            row("2026-01-02", &[("v__a", 2.0), ("v__b", 20.0)]),
        ];
        let out = parse_series(&rows, &measures(), "v__d__day", None);
        assert_eq!(
            out["v.a"],
            vec![
                (NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), 1.0),
                (NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(), 2.0),
            ]
        );
        assert_eq!(out["v.b"].len(), 2);
    }

    /// The whole reason this doesn't `unwrap_or(0.0)`: a missing column is a
    /// measure the query didn't carry, and a zero would reach the forecaster
    /// as a measured collapse.
    #[test]
    fn a_missing_column_contributes_nothing_rather_than_a_zero() {
        let rows = vec![
            row("2026-01-01", &[("v__a", 1.0)]),
            row("2026-01-02", &[("v__a", 2.0), ("v__b", 20.0)]),
        ];
        let out = parse_series(&rows, &measures(), "v__d__day", None);
        assert_eq!(out["v.a"].len(), 2);
        assert_eq!(
            out["v.b"],
            vec![(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(), 20.0)]
        );
    }

    /// A measure that was asked for is always a key, so `project_one` reports
    /// a refusal for it instead of the response simply omitting the measure.
    #[test]
    fn every_requested_measure_is_present_even_with_no_rows() {
        let out = parse_series(&[], &measures(), "v__d__day", None);
        assert_eq!(out.len(), 2);
        assert!(out["v.a"].is_empty());
    }

    #[test]
    fn trims_the_buckets_the_timezone_padding_added() {
        let rows = vec![
            row("2026-01-01", &[("v__a", 1.0)]),
            row("2026-01-02", &[("v__a", 2.0)]),
            row("2026-01-03", &[("v__a", 3.0)]),
        ];
        let trim = Some((
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
            NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(),
        ));
        let out = parse_series(&rows, &measures(), "v__d__day", trim);
        assert_eq!(
            out["v.a"],
            vec![(NaiveDate::from_ymd_opt(2026, 1, 2).unwrap(), 2.0)]
        );
    }

    /// A split retry merges two responses, so arrival order is not bucket
    /// order — and MSTL reads the array as uniformly spaced.
    #[test]
    fn sorts_buckets_that_arrived_out_of_order() {
        let rows = vec![
            row("2026-01-03", &[("v__a", 3.0)]),
            row("2026-01-01", &[("v__a", 1.0)]),
        ];
        let out = parse_series(&rows, &measures(), "v__d__day", None);
        assert_eq!(
            out["v.a"].iter().map(|(_, v)| *v).collect::<Vec<_>>(),
            vec![1.0, 3.0]
        );
    }

    #[test]
    fn a_measure_with_too_little_history_refuses_rather_than_flatlines() {
        let history: Vec<(NaiveDate, f64)> = (0..10)
            .map(|i| {
                (
                    NaiveDate::from_ymd_opt(2026, 1, 1).unwrap() + chrono::Duration::days(i),
                    100.0,
                )
            })
            .collect();
        let out = project_one("v.a", &history, None, Granularity::Day, 7, vec![7]);
        assert!(out.forecast.is_empty());
        assert!(
            out.refusal.as_deref().unwrap_or_default().contains("need"),
            "refusal should name the floor: {:?}",
            out.refusal
        );
        // The history it did have is still returned — the chart draws what
        // happened even when it cannot draw what comes next.
        assert_eq!(out.history.len(), 10);
    }

    #[test]
    fn a_non_finite_bound_serialises_as_null_not_as_the_point() {
        let wire = to_wire(&ProjectedBucket {
            date: NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(),
            point: 5.0,
            lower: f64::NEG_INFINITY,
            upper: f64::INFINITY,
        });
        assert_eq!(wire.lower, None);
        assert_eq!(wire.upper, None);
    }

    #[test]
    fn the_note_fires_only_when_every_measure_is_empty() {
        let all = measures();
        let mut history = SeriesByMeasure::new();
        history.insert("v.a".to_string(), Vec::new());
        history.insert("v.b".to_string(), Vec::new());
        assert!(projection_note(&history, &HashMap::new(), &all).is_some());
        history.insert(
            "v.b".to_string(),
            vec![(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), 1.0)],
        );
        assert!(projection_note(&history, &HashMap::new(), &all).is_none());
    }

    /// A rejected query and an empty window call for opposite fixes, and the
    /// note must not send someone to lengthen a window that was never the
    /// problem.
    #[test]
    fn a_total_query_failure_names_itself_rather_than_blaming_the_window() {
        let failures = measures()
            .into_iter()
            .map(|m| (m, "no such column".to_string()))
            .collect();
        let note = projection_note(&SeriesByMeasure::new(), &failures, &measures())
            .expect("a wholly failed query always has a note");
        assert!(note.contains("no such column"), "{note}");
        assert!(!note.contains("longer window"), "{note}");
    }

    /// The whole point of the per-view retry: one refused measure must not put
    /// a banner over a panel that is drawing a working curve beside it.
    #[test]
    fn a_partial_failure_carries_no_banner() {
        let failures = HashMap::from([("v.b".to_string(), "refused".to_string())]);
        let history = SeriesByMeasure::from([(
            "v.a".to_string(),
            vec![(NaiveDate::from_ymd_opt(2026, 1, 1).unwrap(), 1.0)],
        )]);
        assert_eq!(projection_note(&history, &failures, &measures()), None);
    }

    /// A measure whose query failed has no history, so "not enough history to
    /// fit" would be true and useless — it points at the wrong problem.
    #[test]
    fn a_failed_measure_reports_the_query_not_the_fit() {
        let out = project_one("v.a", &[], Some("bad column"), Granularity::Day, 7, vec![7]);
        assert!(out.history.is_empty());
        assert!(out.forecast.is_empty());
        let refusal = out.refusal.unwrap_or_default();
        assert!(refusal.contains("bad column"), "{refusal}");
        assert!(!refusal.contains("history"), "{refusal}");
    }

    #[test]
    fn groups_measures_by_view_in_first_seen_order() {
        let measures = [
            "checks.a".to_string(),
            "store_days.b".to_string(),
            "checks.c".to_string(),
        ];
        assert_eq!(
            group_by_view(&measures),
            vec![
                vec!["checks.a".to_string(), "checks.c".to_string()],
                vec!["store_days.b".to_string()],
            ]
        );
    }
    /// Airlayer's mixed-additivity refusal, shortened to the word the matcher
    /// keys on (`non-additive`). The verbatim engine text is pinned in
    /// `metric_tree_baseline`'s tests, which is where the matcher lives.
    const MIXED_ADDITIVITY: &str = "Cannot combine additive and non-additive measures from \
         view 'checks' in one query when a requested dimension requires a one-to-many join \
         into that view.";

    /// Two views, and a `checks` view that mixes additivity — the shape that
    /// makes both a cross-view batch and a within-view batch fail, for two
    /// different reasons.
    fn split_layer() -> oxy_airlayer_compat::SemanticLayer {
        let checks = r#"
name: checks
table: public.checks
dialect: postgres
measures:
  - name: total_guests
    type: sum
    expr: party_size
  - name: net_revenue
    type: custom
    expr: "{{checks.total_guests}}"
"#;
        let store_days = r#"
name: store_days
table: public.store_days
dialect: postgres
measures:
  - name: net_sales
    type: sum
    expr: net_sales
"#;
        oxy_airlayer_compat::SemanticLayer::new(
            vec![
                oxy_airlayer_compat::parse_view_yaml(checks).unwrap(),
                oxy_airlayer_compat::parse_view_yaml(store_days).unwrap(),
            ],
            None,
        )
    }

    const SERIES_DIM: &str = "checks__check_date__day";

    fn series_request(measures: &[&str]) -> oxy_airlayer_compat::engine::query::QueryRequest {
        oxy_airlayer_compat::engine::query::QueryRequest {
            measures: measures.iter().map(|m| (*m).to_string()).collect(),
            dimensions: vec!["checks.check_date".to_string()],
            ..oxy_airlayer_compat::engine::query::QueryRequest::new()
        }
    }

    /// One row per day carrying every measure the request asked for.
    fn series_rows(measures: &[String]) -> Vec<serde_json::Map<String, serde_json::Value>> {
        ["2026-01-01", "2026-01-02"]
            .iter()
            .map(|day| {
                let mut r = serde_json::Map::new();
                r.insert(SERIES_DIM.to_string(), json!(*day));
                for m in measures {
                    r.insert(m.replace('.', "__"), json!(1.0));
                }
                r
            })
            .collect()
    }

    fn views_of(measures: &[String]) -> Vec<String> {
        let mut views: Vec<String> = measures
            .iter()
            .map(|m| m.split('.').next().unwrap_or(m).to_string())
            .collect();
        views.sort();
        views.dedup();
        views
    }

    /// FINDING 1: the per-view retry re-enters the splitting executor, and that
    /// executor used to discard the group it HAD answered as soon as a sibling
    /// group failed — so `read_series`'s own argument for a partial answer
    /// stopped applying one layer down, and a view lost every curve in it
    /// because one measure in it was refused.
    #[test]
    fn a_refused_half_costs_only_its_own_measures_their_series() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = seen.clone();
        let inner: Box<oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor> = Box::new(
            move |r: &oxy_airlayer_compat::engine::query::QueryRequest| {
                calls.lock().unwrap().push(r.measures.clone());
                // Two views with no join path: refused as a whole, and NOT an
                // additivity refusal, so the split declines it and the per-view
                // retry is what breaks it up.
                if views_of(&r.measures).len() > 1 {
                    return Err(oxy_airlayer_compat::engine::EngineError::QueryError(
                        "No join path found between 'checks' and 'store_days'".to_string(),
                    ));
                }
                let non_additive = r.measures.iter().any(|m| m == "checks.net_revenue");
                let additive = r.measures.iter().any(|m| m != "checks.net_revenue");
                if additive && non_additive {
                    return Err(oxy_airlayer_compat::engine::EngineError::QueryError(
                        MIXED_ADDITIVITY.to_string(),
                    ));
                }
                if non_additive {
                    return Err(oxy_airlayer_compat::engine::EngineError::QueryError(
                        "custom measures broke".to_string(),
                    ));
                }
                Ok(series_rows(&r.measures))
            },
        );
        let executor = PartialSplitExecutor::new(
            split_layer(),
            inner,
            std::time::Instant::now() + std::time::Duration::from_secs(300),
        );

        let measures: Vec<String> = [
            "checks.total_guests",
            "checks.net_revenue",
            "store_days.net_sales",
        ]
        .iter()
        .map(|m| (*m).to_string())
        .collect();
        let (series, failures) = read_series(
            &executor,
            &series_request(&[
                "checks.total_guests",
                "checks.net_revenue",
                "store_days.net_sales",
            ]),
            &measures,
            SERIES_DIM,
            None,
        );

        assert_eq!(series["checks.total_guests"].len(), 2, "{series:?}");
        assert_eq!(series["store_days.net_sales"].len(), 2, "{series:?}");
        // The refused measure gets no series AT ALL rather than an empty one:
        // an empty series with no refusal beside it is what renders as 0.
        assert!(!series.contains_key("checks.net_revenue"), "{series:?}");
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(
            failures["checks.net_revenue"].contains("custom measures broke"),
            "{failures:?}"
        );
        // And a view whose group failed must not stop the views after it.
        let requests = seen.lock().unwrap().clone();
        assert!(
            requests
                .iter()
                .any(|m| m == &vec!["store_days.net_sales".to_string()]),
            "the second view is still asked for: {requests:?}"
        );
    }

    /// FINDING 2: the budget is checked before ANY query this executor is
    /// asked to run — including the very first batched attempt `read_series`
    /// makes, not only the per-view retries it falls back to. So a deadline
    /// spent before `read_series` even starts must produce no warehouse call
    /// at all: the batched attempt is refused before `inner` ever runs, the
    /// retry loop reads the SAME clock and skips its own queries too, and
    /// every measure is attributed to the budget rather than to a warehouse
    /// that was never asked.
    #[test]
    fn a_spent_budget_stops_the_retries_and_names_itself() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = seen.clone();
        let inner: Box<oxy_airlayer_compat::engine::metric_tree_ops::QueryExecutor> = Box::new(
            move |r: &oxy_airlayer_compat::engine::query::QueryRequest| {
                calls.lock().unwrap().push(r.measures.clone());
                Err(oxy_airlayer_compat::engine::EngineError::QueryError(
                    "connection refused".to_string(),
                ))
            },
        );
        // A deadline of `now` stands in for "already spent".
        let executor = PartialSplitExecutor::new(split_layer(), inner, std::time::Instant::now());

        let measures: Vec<String> = ["checks.total_guests", "store_days.net_sales"]
            .iter()
            .map(|m| (*m).to_string())
            .collect();
        let (series, failures) = read_series(
            &executor,
            &series_request(&["checks.total_guests", "store_days.net_sales"]),
            &measures,
            SERIES_DIM,
            None,
        );

        assert!(series.is_empty(), "{series:?}");
        assert_eq!(
            seen.lock().unwrap().len(),
            0,
            "no query at all, not even the batched attempt; the budget is spent \
             before `inner` is ever called"
        );
        for measure in &measures {
            let reason = &failures[measure];
            assert!(reason.contains("budget"), "{reason}");
            assert_eq!(reason, BUDGET_SPENT, "the refusal is the shared sentence");
        }
    }
}
