//! Concrete `MetricTreeRunner` for Oxy.
//!
//! Loads the workspace's semantic model from disk, compiles airlayer
//! `QueryRequest`s through `SemanticEngine`, and executes the resulting SQL
//! through Oxy's connector pool. Used by both the HTTP `/semantic/metric-tree`
//! handlers and the agentic analytics tools — same code path, identical
//! resolution semantics.
//!
//! Per-run cost notes (see `metric-tree.md` for the algorithmic context): a
//! single `explain` call can fire 100+ queries against the warehouse. Each
//! query goes through `Handle::block_on` because airlayer's `QueryExecutor`
//! type is synchronous; the wrapping `spawn_blocking` is provided by the
//! caller (HTTP handler) so the runtime thread isn't blocked.

use std::sync::{Arc, RwLock};

use agentic_analytics::{MetricTreeRunner, MetricTreeRunnerError};
use agentic_semantic::refresh_key_cache::RefreshKeyCache;
use async_trait::async_trait;
use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy_airlayer_compat::DatabaseConfig;
use oxy_airlayer_compat::SemanticLayer;
use oxy_airlayer_compat::engine::EngineError;
use oxy_airlayer_compat::engine::metric_tree::MetricTree;
use oxy_airlayer_compat::engine::metric_tree_ops::{
    self, ExplainConfig, ExplainResult, OpportunityResult, QueryExecutor,
};
use oxy_airlayer_compat::engine::query::{QueryFilter, QueryRequest};
use serde_json::{Map, Value};
use uuid::Uuid;

use crate::agentic_wiring::project_ctx::build_connector_for_db;
// A blocking thread is off the request's task, so the preview hold is passed
// in (`holds_writes`) rather than read from `write_hold::active()` there.
use crate::server::api::metric_tree::OPPORTUNITY_MIN_SUPPORT;
use crate::server::preagg_context::RollupFreshness;
use oxy::config::{ReadOnly, ScanDir, WorkingCopy};
use oxy_app_core::write_hold::connector::Session;
use oxy_app_core::write_hold::hold_if;
use oxy_metric_monitoring::metric_tree_series::{build_time_series_query_request, trim_to_window};

/// Adapter that exposes Oxy's semantic-model + connector pool as a
/// [`MetricTreeRunner`].
///
/// Holds the smallest set of fields needed to (a) load the on-disk semantic
/// model and (b) execute SQL through `run_via_agentic_connector`. Built once
/// per pipeline run from [`crate::agentic_wiring::OxyProjectContext`].
/// The rollup short-circuit a runner carries: the shared Layer-1 cache, the
/// renewal threshold its freshness check runs against, and what to do with a
/// rollup that check calls stale.
///
/// One struct rather than three arguments because they are only ever correct
/// together — a call site that attached the cache and forgot the threshold got
/// 120s while the rebuild cycle honoured the workspace's own setting, and one
/// that forgot the freshness posture got a read surface's tolerance on a path
/// that writes anomalies to the Insights Inbox.
///
/// `Default` is the no-rollup posture: no cache, so every query compiles to
/// warehouse SQL.
#[derive(Clone, Default)]
pub struct RunnerPreagg {
    pub cache: Option<Arc<RwLock<RefreshKeyCache>>>,
    pub renewal_threshold_secs: u64,
    pub freshness: RollupFreshness,
}

impl RunnerPreagg {
    /// This short-circuit, or a cache-less copy of it when the request buckets
    /// in a non-UTC timezone.
    ///
    /// Rollups are built UTC-truncated, and airlayer's rollup match predicate
    /// considers granularity but not timezone — so a tz'd query served from a
    /// rollup would get silently UTC-bucketed data. Withholding the cache makes
    /// the executor take the warehouse path, which is correct by construction.
    /// UTC monitors keep their rollups. (Timezone-aware rollups are an upstream
    /// airlayer change.)
    fn for_timezone(&self, timezone: Option<&str>) -> Self {
        match timezone {
            Some(tz) if tz != "UTC" => Self {
                cache: None,
                ..self.clone()
            },
            _ => self.clone(),
        }
    }
}

pub struct OxyMetricTreeRunner {
    workspace_manager: WorkspaceManager<ReadOnly>,
    user_id: Uuid,
    role: WorkspaceRole,
    preagg: RunnerPreagg,
    /// Memoized file-level `.monitor.yml` timezone, read on first use.
    default_timezone: std::sync::OnceLock<Option<String>>,
    /// When set, the semantic model is read from this scan instead of
    /// `config_manager.semantics_scan_path()`. Customer-app requests run on the
    /// stateless serve fleet, where the workspace FS scan path does not exist —
    /// they read the compiled revision's files from the compile boundary and
    /// point the runner at that.
    scan_root: ScanRoot,
    /// The shared engine cache, when the caller could reach one.
    ///
    /// `None` on the paths that build a runner without an `AppState` to take
    /// it from — the scheduler / cron construction in
    /// `project_ctx::project_context` — which then build the engine per run as
    /// before.
    ///
    /// The `Option<Uuid>` is the revision the runner's SCAN PATH was
    /// materialised from, `None` for the working copy — NOT
    /// `ConfigManager::revision_id()`, which reports the pin and is `Some` even
    /// when reading the working copy.
    engine_cache: Option<(
        Arc<crate::server::router::workspace_cache::SemanticEngineCache>,
        Option<Uuid>,
    )>,
    /// Hold every connector the runner builds (a workspace-preview request).
    /// Read when the runner is built: its queries run on blocking threads,
    /// off the request's task.
    holds_writes: bool,
}

impl OxyMetricTreeRunner {
    pub fn new(
        workspace_manager: WorkspaceManager<ReadOnly>,
        user_id: Uuid,
        role: WorkspaceRole,
    ) -> Self {
        Self {
            workspace_manager,
            user_id,
            role,
            preagg: RunnerPreagg::default(),
            default_timezone: std::sync::OnceLock::new(),
            scan_root: ScanRoot::WorkingCopy,
            engine_cache: None,
            holds_writes: oxy_app_core::write_hold::active(),
        }
    }

    /// Hold every connector this runner builds when `hold` — for a runner
    /// built off the request's task for a context that holds writes.
    pub fn holding_writes(mut self, hold: bool) -> Self {
        self.holds_writes |= hold;
        self
    }

    /// Share the process-wide engine cache.
    ///
    /// `source_revision` is the revision this runner's scan path was
    /// materialised from, or `None` when it reads the working copy — see
    /// `QueryScanSource::source_revision`. Passing the pinned revision instead
    /// would let a working-copy run and a compiled-revision run share an
    /// engine.
    ///
    /// Without this the runner revalidates the layer and rebuilds the join
    /// graph on every explain / opportunity run.
    pub fn with_engine_cache(
        mut self,
        cache: Arc<crate::server::router::workspace_cache::SemanticEngineCache>,
        source_revision: Option<Uuid>,
    ) -> Self {
        self.engine_cache = Some((cache, source_revision));
        self
    }

    pub fn with_preagg(
        mut self,
        cache: Option<Arc<RwLock<RefreshKeyCache>>>,
        renewal_threshold_secs: u64,
    ) -> Self {
        self.preagg.cache = cache;
        self.preagg.renewal_threshold_secs = renewal_threshold_secs;
        self
    }

    /// Attach an already-assembled short-circuit, freshness posture included.
    pub fn with_preagg_ctx(mut self, preagg: RunnerPreagg) -> Self {
        self.preagg = preagg;
        self
    }

    /// Decline a rollup the freshness check calls stale rather than serve it.
    ///
    /// For the anomaly scan and its explain, which persist what they compute
    /// to the Insights Inbox: a rollup whose newest buckets are missing reads
    /// as a drop, and an unhealthy transition pages Slack. See
    /// `PreaggContext::require_fresh` for the full argument.
    pub fn requiring_fresh_rollups(mut self) -> Self {
        self.preagg.freshness = RollupFreshness::RequireFresh;
        self
    }

    /// The workspace's default bucketing timezone. Read at most once per
    /// runner. `None` means UTC.
    fn default_timezone(&self) -> Option<String> {
        self.default_timezone
            .get_or_init(|| {
                self.workspace_manager
                    .config_manager
                    .working_copy()
                    .and_then(|wc| read_default_timezone(wc.root()))
            })
            .clone()
    }

    /// The engine for `layer` — from the shared cache when this runner was
    /// given one, otherwise built for this run.
    ///
    /// The layer is the workspace's own, unmodified: `run_explain` prunes
    /// dimensions AFTER this point, against `RunInputs.layer`, so the cached
    /// engine is not the pruned one and two runs pruning differently still
    /// share it.
    fn engine_for(
        &self,
        layer: &SemanticLayer,
        databases: &[DatabaseConfig],
    ) -> Result<Arc<oxy_airlayer_compat::SemanticEngine>, MetricTreeRunnerError> {
        let build = || oxy_airlayer_compat::build_engine(layer.clone(), databases);
        match &self.engine_cache {
            Some((cache, source_revision)) => {
                let key = oxy_airlayer_compat::EngineKey::for_source(
                    self.workspace_manager.workspace_id,
                    *source_revision,
                    databases,
                );
                cache
                    .get_or_build(key, build)
                    .map_err(|e| MetricTreeRunnerError::ExecutorBuild(e.to_string()))
            }
            None => build()
                .map(Arc::new)
                .map_err(|e| MetricTreeRunnerError::ExecutorBuild(e.to_string())),
        }
    }

    /// Read the semantic model from `scan` instead of the workspace FS scan
    /// path — the compile-boundary path for the stateless serve fleet.
    pub fn with_scan(mut self, scan: ScanDir) -> Self {
        self.scan_root = ScanRoot::Compiled(scan);
        self
    }

    /// Refuse this node's working copy outright: with no compile-boundary root,
    /// answer with nothing rather than with whatever is on disk here. See
    /// [`ScanRoot::Unavailable`].
    pub fn without_working_copy_fallback(mut self) -> Self {
        if matches!(self.scan_root, ScanRoot::WorkingCopy) {
            self.scan_root = ScanRoot::Unavailable;
        }
        self
    }

    /// Where the semantic model is read from — see [`ScanRoot`].
    /// `None` means there is nothing this runner is allowed to read.
    fn effective_scan(&self) -> Option<ScanDir> {
        match &self.scan_root {
            ScanRoot::Compiled(scan) => Some(scan.clone()),
            ScanRoot::Unavailable => None,
            ScanRoot::WorkingCopy => self
                .workspace_manager
                .config_manager
                .working_copy()
                .map(|wc| wc.root().to_path_buf())
                // The slot being FULL does not mean the files are here:
                // `effective_workspace_path` returns the database column
                // without stat-ing it, so on a stateless replica this arm
                // always yielded a path and airlayer reported the workspace
                // root as an I/O fault (`failed to read <root>: No such file
                // or directory`). That reads as a broken workspace. `None`
                // instead, so the caller's error names the real cause: no scan
                // root was resolved on this node.
                .filter(|root| root.is_dir())
                .map(ScanDir::Real),
        }
    }

    /// Parse a semantic model from an ALREADY-RESOLVED scan.
    ///
    /// Deliberately takes the scan rather than a `WorkspaceManager`: it used to
    /// derive the root from `config_manager.semantics_scan_path()` itself, which
    /// is the workspace working copy — a directory that does not exist on a
    /// stateless serve replica, so every metric-tree call 500'd there
    /// (oxy-hq/oxygen#878). Callers now resolve the root through the compile
    /// boundary first (`semantic::resolve_query_scan_source`) and pass it in.
    pub fn load_layer_at(scan: &ScanDir) -> Result<SemanticLayer, MetricTreeRunnerError> {
        scan.load_layer()
            .map_err(|e| MetricTreeRunnerError::LayerLoad(e.to_string()))
    }

    /// List configured databases as airlayer `DatabaseConfig`s. The
    /// `DatasourceDialectMap` is built from these by the caller.
    pub fn list_databases_sync<S: oxy::config::DiskSlot>(
        workspace_manager: &WorkspaceManager<S>,
    ) -> Vec<DatabaseConfig> {
        workspace_manager
            .config_manager
            .list_databases()
            .iter()
            .map(|db| DatabaseConfig {
                name: db.name.clone(),
                // `dialect()`, not the raw type name: airhouse and motherduck
                // speak an engine their `type:` string does not name, and
                // airlayer drops a datasource it cannot classify -- silently
                // inheriting whichever dialect config.yml lists first.
                db_type: db.dialect(),
            })
            .collect()
    }
}

#[async_trait]
impl MetricTreeRunner for OxyMetricTreeRunner {
    async fn load_layer(&self) -> Result<SemanticLayer, MetricTreeRunnerError> {
        let scan = self.effective_scan().ok_or_else(|| {
            MetricTreeRunnerError::LayerLoad(self.scan_root.unavailable_reason().to_string())
        })?;
        scan.load_layer()
            .map_err(|e| MetricTreeRunnerError::LayerLoad(e.to_string()))
    }

    async fn list_databases(&self) -> Vec<DatabaseConfig> {
        Self::list_databases_sync(&self.workspace_manager)
    }

    async fn run_explain(
        &self,
        target: String,
        time_dimension: String,
        current_period: (String, String),
        previous_period: (String, String),
        filters: Vec<QueryFilter>,
        config: ExplainConfig,
    ) -> Result<ExplainResult, MetricTreeRunnerError> {
        let inputs = self.snapshot_for_blocking().await?;
        let total_start = std::time::Instant::now();
        let target_for_log = target.clone();
        // QueryExecutor isn't Send, so build + consume entirely inside
        // spawn_blocking. All inputs (engine, databases, workspace_manager,
        // tokio handle, the period tuples, the base filters) are Send.
        let result = tokio::task::spawn_blocking(move || {
            let RunInputs {
                layer,
                databases,
                engine,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg,
                holds_writes,
            } = inputs;
            // Members pinned by the base filter (the monitor's group_by /
            // segment) carry no signal as split candidates once we scope to
            // them — exclude them along with the time dimension, numeric,
            // seasonal, and row-key dims. See [`prune_dims_for_explain`].
            let exclude_members: Vec<String> =
                filters.iter().filter_map(|f| f.member.clone()).collect();
            let pruned = prune_dims_for_explain(layer, &exclude_members, &time_dimension);
            let tree = MetricTree::build(&pruned);
            let inner = build_query_executor(
                engine,
                databases,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg,
                holds_writes,
            );
            // Scope every query the explain issues to the anomaly's segment by
            // appending the base filters before delegating to the executor.
            // airlayer's `explain` has no base-filter hook, so we inject here;
            // the result cache keys on compiled SQL, so scoped queries never
            // collide with unscoped ones.
            let executor: Box<QueryExecutor> = if filters.is_empty() {
                inner
            } else {
                Box::new(move |request: &QueryRequest| {
                    let mut scoped = request.clone();
                    scoped.filters.extend(filters.iter().cloned());
                    inner(&scoped)
                })
            };
            metric_tree_ops::explain(
                &tree,
                &pruned,
                &target,
                &time_dimension,
                (current_period.0.as_str(), current_period.1.as_str()),
                (previous_period.0.as_str(), previous_period.1.as_str()),
                &config,
                &*executor,
            )
            .map_err(|e| MetricTreeRunnerError::Op(e.to_string()))
        })
        .await
        .map_err(|e| MetricTreeRunnerError::Op(format!("explain task panicked: {e}")))?;
        tracing::info!(
            target: "metric_tree.explain",
            measure = %target_for_log,
            total_ms = total_start.elapsed().as_millis() as u64,
            ok = result.is_ok(),
            "explain done"
        );
        result
    }

    async fn get_dimension_values(
        &self,
        dimension: String,
        measure: String,
        since_days: u32,
    ) -> Result<Vec<String>, MetricTreeRunnerError> {
        use oxy_airlayer_compat::engine::query::{QueryRequest, TimeDimensionQuery};
        let inputs = self.snapshot_for_blocking().await?;
        // Derive the time dimension from the measure's view: take the view
        // prefix from `measure` (e.g. "sales_daily") and append the first
        // available date-type dimension for the lookback filter. We send a
        // dimensions-only query (no measures) so airlayer returns raw rows
        // grouped by the requested dimension. The date range prunes stale
        // segments (closed locations etc.) so only active values come back.
        let since = chrono::Utc::now() - chrono::Duration::days(since_days as i64);
        let since_str = since.format("%Y-%m-%d").to_string();
        let now_str = chrono::Utc::now().format("%Y-%m-%d").to_string();

        // Infer the view's primary time dimension from the measure id.
        // Convention: `view_name.measure` → time dim is `view_name.business_date`
        // or `view_name.created_at`. We try `business_date` first (Toast views),
        // then fall back to omitting the date filter (returns all-time values).
        let view = measure.split('.').next().unwrap_or("").to_string();
        let time_dim_candidate = format!("{view}.business_date");

        let dim_alias = dimension.replace('.', "__");

        let request_with_date = QueryRequest {
            dimensions: vec![dimension.clone()],
            time_dimensions: vec![TimeDimensionQuery {
                dimension: time_dim_candidate.clone(),
                granularity: None,
                date_range: Some(vec![since_str.clone(), now_str.clone()]),
            }],
            limit: Some(10_000),
            ..QueryRequest::new()
        };
        let request_no_date = QueryRequest {
            dimensions: vec![dimension.clone()],
            limit: Some(10_000),
            ..QueryRequest::new()
        };

        tokio::task::spawn_blocking(move || {
            let RunInputs {
                databases,
                engine,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg,
                holds_writes,
                ..
            } = inputs;
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
            // Try with date filter first; fall back to unfiltered on error
            // (some views don't have a `business_date` dimension).
            let rows = (executor)(&request_with_date)
                .or_else(|_| (executor)(&request_no_date))
                .map_err(|e| MetricTreeRunnerError::Op(e.to_string()))?;
            let mut out: Vec<String> = Vec::with_capacity(rows.len());
            for row in rows {
                if let Some(v) = row.get(&dim_alias).and_then(|v| {
                    v.as_str()
                        .map(String::from)
                        .or_else(|| Some(v.to_string().trim_matches('"').to_string()))
                }) && !v.is_empty()
                    && v != "null"
                {
                    out.push(v);
                }
            }
            out.sort();
            out.dedup();
            Ok(out)
        })
        .await
        .map_err(|e| MetricTreeRunnerError::Op(format!("dimension-values task panicked: {e}")))?
    }

    async fn run_time_series(
        &self,
        measure: String,
        time_dimension: String,
        granularity: String,
        period: (String, String),
        filters: Vec<oxy_airlayer_compat::engine::query::QueryFilter>,
        timezone: Option<String>,
    ) -> Result<Vec<(String, f64)>, MetricTreeRunnerError> {
        // `None` from the caller means "workspace default", not "UTC" — this is
        // how the analytics agent's detect_anomalies tool inherits the same
        // timezone the scheduled scans use.
        let timezone = timezone.or_else(|| self.default_timezone());
        // airlayer applies the `date_range` WHERE clause to the RAW,
        // unconverted column while the SELECT buckets the timezone-*converted*
        // one, so a non-UTC request clips or partially-sums its first and
        // last local buckets. When converting, widen the requested range and
        // trim the extra rows back off below — this is the single place every
        // caller (scheduled scans and the chat `detect_anomalies` tool alike)
        // gets the workaround for free. UTC requests have no conversion, so
        // no clipping, and take the pre-existing unwidened path byte-for-byte.
        // `build_time_series_query_request` is the one seam that decides both
        // the request and the trim window, so it — not this function body —
        // is what a regression here would have to touch.
        let (request, trim_window) = build_time_series_query_request(
            std::slice::from_ref(&measure),
            &time_dimension,
            &granularity,
            filters,
            &period,
            timezone.clone(),
        );
        let inputs = self.snapshot_for_blocking().await?;
        let measure_alias = measure.replace('.', "__");
        let dim_alias_for_extract = format!("{time_dimension}.{granularity}").replace('.', "__");
        let rows = tokio::task::spawn_blocking(move || {
            let RunInputs {
                databases,
                engine,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg,
                holds_writes,
                ..
            } = inputs;
            let executor = build_query_executor(
                engine,
                databases,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg.for_timezone(timezone.as_deref()),
                holds_writes,
            );
            let rows =
                (executor)(&request).map_err(|e| MetricTreeRunnerError::Op(e.to_string()))?;
            // airlayer returns column-keyed maps with the measure aliased as
            // `view__measure` and the time bucket as `view__dim.granularity`
            // (dotted). The connector flattens dots in keys, so use the
            // pre-flattened aliases for lookups.
            let mut out: Vec<(String, f64)> = Vec::with_capacity(rows.len());
            for row in rows {
                let ts = row
                    .get(&dim_alias_for_extract)
                    .and_then(|v| v.as_str().map(String::from))
                    .or_else(|| {
                        row.get(&dim_alias_for_extract)
                            .map(|v| v.to_string().trim_matches('"').to_string())
                    })
                    .unwrap_or_default();
                let value = row
                    .get(&measure_alias)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                if !ts.is_empty() {
                    out.push((ts, value));
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| MetricTreeRunnerError::Op(format!("time-series task panicked: {e}")))??;
        Ok(match trim_window {
            Some((start, end)) => trim_to_window(rows, start, end),
            None => rows,
        })
    }

    async fn run_query_scalar(
        &self,
        request: oxy_airlayer_compat::engine::query::QueryRequest,
    ) -> Result<f64, MetricTreeRunnerError> {
        let measure = request.measures.first().cloned().ok_or_else(|| {
            MetricTreeRunnerError::Op("run_query_scalar: request has no measure".to_string())
        })?;
        let inputs = self.snapshot_for_blocking().await?;
        let measure_alias = measure.replace('.', "__");
        tokio::task::spawn_blocking(move || {
            let RunInputs {
                databases,
                engine,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg,
                holds_writes,
                ..
            } = inputs;
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
            let rows =
                (executor)(&request).map_err(|e| MetricTreeRunnerError::Op(e.to_string()))?;
            // Single aggregate row; the measure is aliased `view__measure`. An
            // empty window (no rows) reads as 0.0 — a SUM/COUNT of nothing.
            let value = rows
                .first()
                .and_then(|row| row.get(&measure_alias))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            Ok(value)
        })
        .await
        .map_err(|e| MetricTreeRunnerError::Op(format!("scalar task panicked: {e}")))?
    }

    async fn run_query_grouped(
        &self,
        request: oxy_airlayer_compat::engine::query::QueryRequest,
        keys: Vec<String>,
    ) -> Result<Vec<(Vec<String>, f64)>, MetricTreeRunnerError> {
        let measure = request.measures.first().cloned().ok_or_else(|| {
            MetricTreeRunnerError::Op("run_query_grouped: request has no measure".to_string())
        })?;
        let inputs = self.snapshot_for_blocking().await?;
        // Same aliasing as the scalar and time-series paths: `view.member`
        // comes back keyed `view__member`, a bucketed time dimension
        // `view__member__day`.
        let measure_alias = measure.replace('.', "__");
        let dim_aliases: Vec<String> = keys.iter().map(|k| k.replace('.', "__")).collect();
        tokio::task::spawn_blocking(move || {
            let RunInputs {
                databases,
                engine,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg,
                holds_writes,
                ..
            } = inputs;
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
            let rows =
                (executor)(&request).map_err(|e| MetricTreeRunnerError::Op(e.to_string()))?;
            Ok(rows
                .iter()
                .map(|row| {
                    let key = dim_aliases
                        .iter()
                        .map(|alias| match row.get(alias) {
                            None | Some(Value::Null) => "null".to_string(),
                            Some(Value::String(s)) => s.clone(),
                            Some(v) => v.to_string(),
                        })
                        .collect();
                    let value = row
                        .get(&measure_alias)
                        .and_then(Value::as_f64)
                        .unwrap_or(0.0);
                    (key, value)
                })
                .collect())
        })
        .await
        .map_err(|e| MetricTreeRunnerError::Op(format!("grouped task panicked: {e}")))?
    }

    async fn run_opportunity(
        &self,
        target: String,
        time_dimension: String,
        period: (String, String),
        statistic: oxy_airlayer_compat::engine::metric_tree_ops::BenchmarkStatistic,
    ) -> Result<OpportunityResult, MetricTreeRunnerError> {
        let inputs = self.snapshot_for_blocking().await?;
        tokio::task::spawn_blocking(move || {
            let RunInputs {
                layer,
                databases,
                engine,
                workspace_manager,
                user_id,
                role,
                handle,
                preagg,
                holds_writes,
            } = inputs;
            let tree = MetricTree::build(&layer);
            // Off the `databases` the executor queries, so the values and the
            // dialect they are judged by come from one warehouse.
            let dialect =
                oxy_airlayer_compat::predict_dialect(&layer, &databases, &tree, &[target.as_str()]);
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
            metric_tree_ops::opportunity(
                &tree,
                &layer,
                &target,
                &time_dimension,
                (period.0.as_str(), period.1.as_str()),
                // The agent asks a population-level question ("where is the
                // upside in this measure?"); it has no instance in focus to
                // narrow to.
                &[],
                statistic,
                OPPORTUNITY_MIN_SUPPORT,
                dialect,
                &*executor,
            )
            .map_err(|e| MetricTreeRunnerError::Op(e.to_string()))
        })
        .await
        .map_err(|e| MetricTreeRunnerError::Op(format!("opportunity task panicked: {e}")))?
    }
}

/// All the `Send` inputs needed to construct a `QueryExecutor` inside
/// `spawn_blocking`. Kept as a struct so the two query-executing ops can
/// share the same snapshot path without duplicating six trailing locals.
struct RunInputs {
    layer: SemanticLayer,
    databases: Vec<DatabaseConfig>,
    engine: Arc<oxy_airlayer_compat::SemanticEngine>,
    workspace_manager: WorkspaceManager<ReadOnly>,
    user_id: Uuid,
    role: WorkspaceRole,
    handle: tokio::runtime::Handle,
    preagg: RunnerPreagg,
    holds_writes: bool,
}

impl OxyMetricTreeRunner {
    async fn snapshot_for_blocking(&self) -> Result<RunInputs, MetricTreeRunnerError> {
        let layer = self.load_layer().await?;
        let databases = self.list_databases().await;
        let engine = self.engine_for(&layer, &databases)?;
        Ok(RunInputs {
            layer,
            databases,
            engine,
            workspace_manager: self.workspace_manager.clone(),
            user_id: self.user_id,
            role: self.role.clone(),
            handle: tokio::runtime::Handle::current(),
            preagg: self.preagg.clone(),
            holds_writes: self.holds_writes,
        })
    }
}

/// File-level `timezone` from a workspace's `.monitor.yml`, or `None` when the
/// file is absent or unreadable. Deliberately lossy: a malformed monitor config
/// must not break unrelated metric-tree queries — the scan path reports the
/// real parse error.
pub(crate) fn read_default_timezone(workspace_root: &std::path::Path) -> Option<String> {
    let path = oxy_metric_monitoring::default_config_path(workspace_root);
    match oxy_metric_monitoring::load_from_file(&path) {
        Ok(cfg) => cfg.timezone,
        Err(e) => {
            tracing::debug!(
                target: "metric_monitoring",
                error = %e,
                "could not read a default timezone from .monitor.yml; using UTC"
            );
            None
        }
    }
}

/// Build a `QueryExecutor` closure: compile each `QueryRequest` to SQL via
/// airlayer, then execute through Oxy's connector. The closure runs on a
/// blocking thread, so `block_on` bridges to the async connector call.
///
/// **Connectors are cached for the lifetime of the closure.** Explain calls
/// fire 10-50 queries against the same database; without the cache each
/// query was building a fresh `OxyProjectContext` and re-opening the
/// DuckDB file. With the cache, the connector is built once on first use
/// and reused across every query, which on the orders fixture cuts an
/// explain run from ~10s down to <2s.
///
/// Extracted here so both `OxyMetricTreeRunner` and the HTTP `/explain` and
/// `/opportunity` handlers go through the exact same code path.
#[allow(clippy::too_many_arguments)]
pub fn build_query_executor(
    engine: Arc<oxy_airlayer_compat::SemanticEngine>,
    databases: Vec<DatabaseConfig>,
    workspace_manager: WorkspaceManager<ReadOnly>,
    user_id: Uuid,
    role: WorkspaceRole,
    handle: tokio::runtime::Handle,
    preagg: RunnerPreagg,
    holds_writes: bool,
) -> Box<QueryExecutor> {
    use agentic_connector::DatabaseConnector;

    // The pre-aggregation short-circuit — DERIVED from the workspace manager,
    // never taken from the caller. Its cache key is the workspace ID, so a
    // materialised compile-boundary tempdir or a `.worktrees/<branch>`
    // checkout in scope at a call site can no longer be mistaken for it: both
    // used to hash to a directory nothing had ever built, and every query then
    // silently took the warehouse path — right rows, no "Pre-aggregated"
    // badge, none of the explain headroom.
    let preagg_ctx = crate::server::preagg_context::preagg_context(
        workspace_manager.workspace_id,
        preagg.cache,
        Some(preagg.renewal_threshold_secs),
        preagg.freshness,
    );

    // What the loaded schema still declares, computed once here rather than
    // per query: the closure below runs a few hundred times in one explain,
    // and this walks every view's `pre_aggregations:`. Owned so it can move
    // into the closure alongside `engine`, which it is derived from.
    let live_rollups = {
        let views: Vec<&oxy_airlayer_compat::View> = engine.semantic_layer().views.iter().collect();
        oxy_airlayer_compat::live_rollups_or_decline(&views)
    };

    let pool: std::sync::Mutex<std::collections::HashMap<String, Vec<Arc<dyn DatabaseConnector>>>> =
        std::sync::Mutex::new(std::collections::HashMap::new());

    // Per-explain result cache: (database, sql) → rows. airlayer's
    // explain re-queries the same component measures during opposing-
    // offset detection that it already pulled in evaluate_candidates,
    // so the same SQL fires 2-3 times in a typical run. Memoize within
    // a single explain to skip the duplicates. (Upstream airlayer fix
    // tracked in metric-tree.md.)
    let result_cache: std::sync::Mutex<
        std::collections::HashMap<(String, String), Vec<Map<String, Value>>>,
    > = std::sync::Mutex::new(std::collections::HashMap::new());
    // Per-explain query counter for the trace summary. Each call increments
    // it so log lines can be correlated across the recursive search.
    let query_seq = std::sync::atomic::AtomicUsize::new(0);
    Box::new(move |request: &QueryRequest| {
        let compiled = engine.compile_query(request)?;
        // airlayer emits parameterized SQL ($1, $2, …) + a separate params
        // vector. The agentic-connector executor takes a raw SQL string, so
        // inline the params the same way `resolve_and_compile` does.
        let sql = oxy_airlayer_compat::substitute_params(
            &oxy_airlayer_compat::request_dialect(&engine, request),
            &compiled.sql,
            &compiled.params,
        );
        let database = resolve_database(&engine, request, &databases)?;
        let seq = query_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let total_start = std::time::Instant::now();

        // Result-cache hit? Save the round-trip + warehouse work entirely.
        let result_key = (database.clone(), sql.clone());
        if let Some(cached) = result_cache.lock().unwrap().get(&result_key).cloned() {
            tracing::info!(
                target: "metric_tree.explain",
                seq,
                database = %database,
                result_cache_hit = true,
                row_count = cached.len(),
                total_ms = total_start.elapsed().as_millis() as u64,
                "query (cached)"
            );
            return Ok(cached);
        }

        // Preagg path: if a covering rollup exists in the local Parquet manifest,
        // serve from DuckDB instead of the warehouse. Any failure falls through
        // silently to the warehouse path below.
        if let Some(ref preagg) = preagg_ctx
            && let Some(agentic_semantic::compile::CompiledQuery::Preaggregation {
                preagg_sql,
                source,
                ..
            }) = agentic_semantic::compile::try_resolve_preagg(
                preagg,
                request,
                &sql,
                &database,
                Some(&live_rollups),
            )
        {
            match execute_preagg_and_convert(&preagg_sql, &source) {
                Ok(rows) => {
                    tracing::info!(
                        target: "metric_tree.explain",
                        seq,
                        database = %database,
                        preagg = true,
                        row_count = rows.len(),
                        total_ms = total_start.elapsed().as_millis() as u64,
                        "query (preagg)"
                    );
                    result_cache
                        .lock()
                        .unwrap()
                        .insert(result_key, rows.clone());
                    return Ok(rows);
                }
                Err(e) => {
                    tracing::warn!(
                        target: "metric_tree.explain",
                        seq,
                        %e,
                        "preagg execute failed, falling back to warehouse"
                    );
                }
            }
        }

        // Pop an idle connector from the pool, or build a fresh one.
        // Threads that find the pool empty build in parallel — concurrent
        // first-batch queries each pay the build cost once, not N times.
        let pooled = pool
            .lock()
            .unwrap()
            .get_mut(&database)
            .and_then(|v| v.pop());
        let (connector, pool_hit) = if let Some(c) = pooled {
            (c, true)
        } else {
            let build_start = std::time::Instant::now();
            let built = handle
                .block_on(build_connector_for_db(
                    &workspace_manager,
                    &database,
                    Some(user_id),
                    Some(role.clone()),
                ))
                .map(|conn| {
                    let session = Session::of(&workspace_manager.config_manager, &database);
                    hold_if(holds_writes, conn, &database, session)
                })
                .map_err(|e| EngineError::QueryError(e.to_string()))?;
            tracing::info!(
                target: "metric_tree.explain",
                seq,
                database = %database,
                build_ms = build_start.elapsed().as_millis() as u64,
                "connector built (pool miss)"
            );
            (built, false)
        };

        let exec_start = std::time::Instant::now();
        let stream = handle
            .block_on(connector.execute_query_full(&sql))
            .map_err(|e| EngineError::QueryError(format!("{e:?}")))?;
        let exec_ms = exec_start.elapsed().as_millis() as u64;
        let decode_start = std::time::Instant::now();
        let rows = handle
            .block_on(crate::server::api::typed_stream::typed_stream_to_json_array(stream))
            .map_err(|e| EngineError::QueryError(format!("{e:?}")))?;
        let decode_ms = decode_start.elapsed().as_millis() as u64;
        let row_count = rows.len();
        let result = rows_to_maps(rows);
        result_cache
            .lock()
            .unwrap()
            .insert(result_key, result.clone());

        // Return the connector to the pool so the next parallel call can reuse it.
        pool.lock()
            .unwrap()
            .entry(database.clone())
            .or_default()
            .push(connector);

        tracing::info!(
            target: "metric_tree.explain",
            seq,
            database = %database,
            pool_hit,
            measures = ?request.measures,
            dimensions = ?request.dimensions,
            row_count,
            exec_ms,
            decode_ms,
            total_ms = total_start.elapsed().as_millis() as u64,
            sql_len = sql.len(),
            "query"
        );
        Ok(result)
    })
}

/// Like [`build_query_executor`], but rebuilds the engine from a SHARED layer on
/// every query instead of capturing one fixed engine. The opportunity drill
/// installs synthetic per-value measures into this same `Arc<RwLock<SemanticLayer>>`
/// mid-recursion (under a brief write lock it always releases before calling the
/// executor), so each query must compile against the CURRENT layer — a frozen
/// engine snapshot would reject a measure it never saw. Per-query rebuild is
/// acceptable: the drill runs on-expand only and issues a bounded number of
/// queries (max_depth × candidates), and results are still cached below.
#[allow(clippy::too_many_arguments)]
pub fn build_drill_query_executor(
    shared_layer: metric_tree_ops::SharedLayer,
    dialects: oxy_airlayer_compat::DatasourceDialectMap,
    databases: Vec<DatabaseConfig>,
    workspace_manager: WorkspaceManager<ReadOnly>,
    user_id: Uuid,
    role: WorkspaceRole,
    handle: tokio::runtime::Handle,
    preagg: RunnerPreagg,
    holds_writes: bool,
) -> Box<QueryExecutor> {
    use agentic_connector::DatabaseConnector;

    // Cache key derived, not passed — same rule as [`build_query_executor`].
    let preagg_ctx = crate::server::preagg_context::preagg_context(
        workspace_manager.workspace_id,
        preagg.cache,
        Some(preagg.renewal_threshold_secs),
        preagg.freshness,
    );

    // Computed once from the shared layer, not per query, even though the
    // engine is rebuilt per query: what the drill mutates mid-recursion is
    // the MEASURE list (synthetic per-value measures), never a view's
    // `pre_aggregations:` block — so the live set cannot move underneath us.
    let live_rollups = {
        let layer = shared_layer.read().expect("shared layer poisoned");
        let views: Vec<&oxy_airlayer_compat::View> = layer.views.iter().collect();
        oxy_airlayer_compat::live_rollups_or_decline(&views)
    };

    let pool: std::sync::Mutex<std::collections::HashMap<String, Vec<Arc<dyn DatabaseConnector>>>> =
        std::sync::Mutex::new(std::collections::HashMap::new());

    // Same per-run result cache + query counter as build_query_executor.
    let result_cache: std::sync::Mutex<
        std::collections::HashMap<(String, String), Vec<Map<String, Value>>>,
    > = std::sync::Mutex::new(std::collections::HashMap::new());
    let query_seq = std::sync::atomic::AtomicUsize::new(0);
    Box::new(move |request: &QueryRequest| {
        // Rebuild the engine from the CURRENT shared layer (read guard dropped
        // immediately after building — the executor holds no layer lock while it
        // runs SQL, and the drill holds none while it calls the executor).
        let engine = {
            let layer = shared_layer.read().expect("shared layer poisoned");
            oxy_airlayer_compat::SemanticEngine::from_semantic_layer(
                layer.clone(),
                dialects.clone(),
            )
            .map_err(|e| EngineError::QueryError(e.to_string()))?
        };
        let compiled = engine.compile_query(request)?;
        // airlayer emits parameterized SQL ($1, $2, …) + a separate params
        // vector. The agentic-connector executor takes a raw SQL string, so
        // inline the params the same way `resolve_and_compile` does.
        let sql = oxy_airlayer_compat::substitute_params(
            &oxy_airlayer_compat::request_dialect(&engine, request),
            &compiled.sql,
            &compiled.params,
        );
        let database = resolve_database(&engine, request, &databases)?;
        let seq = query_seq.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let total_start = std::time::Instant::now();

        // Result-cache hit? Save the round-trip + warehouse work entirely.
        let result_key = (database.clone(), sql.clone());
        if let Some(cached) = result_cache.lock().unwrap().get(&result_key).cloned() {
            tracing::info!(
                target: "metric_tree.explain",
                seq,
                database = %database,
                result_cache_hit = true,
                row_count = cached.len(),
                total_ms = total_start.elapsed().as_millis() as u64,
                "query (cached)"
            );
            return Ok(cached);
        }

        // Preagg path: if a covering rollup exists in the local Parquet manifest,
        // serve from DuckDB instead of the warehouse. Any failure falls through
        // silently to the warehouse path below.
        if let Some(ref preagg) = preagg_ctx
            && let Some(agentic_semantic::compile::CompiledQuery::Preaggregation {
                preagg_sql,
                source,
                ..
            }) = agentic_semantic::compile::try_resolve_preagg(
                preagg,
                request,
                &sql,
                &database,
                Some(&live_rollups),
            )
        {
            match execute_preagg_and_convert(&preagg_sql, &source) {
                Ok(rows) => {
                    tracing::info!(
                        target: "metric_tree.explain",
                        seq,
                        database = %database,
                        preagg = true,
                        row_count = rows.len(),
                        total_ms = total_start.elapsed().as_millis() as u64,
                        "query (preagg)"
                    );
                    result_cache
                        .lock()
                        .unwrap()
                        .insert(result_key, rows.clone());
                    return Ok(rows);
                }
                Err(e) => {
                    tracing::warn!(
                        target: "metric_tree.explain",
                        seq,
                        %e,
                        "preagg execute failed, falling back to warehouse"
                    );
                }
            }
        }

        // Pop an idle connector from the pool, or build a fresh one.
        // Threads that find the pool empty build in parallel — concurrent
        // first-batch queries each pay the build cost once, not N times.
        let pooled = pool
            .lock()
            .unwrap()
            .get_mut(&database)
            .and_then(|v| v.pop());
        let (connector, pool_hit) = if let Some(c) = pooled {
            (c, true)
        } else {
            let build_start = std::time::Instant::now();
            let built = handle
                .block_on(build_connector_for_db(
                    &workspace_manager,
                    &database,
                    Some(user_id),
                    Some(role.clone()),
                ))
                .map(|conn| {
                    let session = Session::of(&workspace_manager.config_manager, &database);
                    hold_if(holds_writes, conn, &database, session)
                })
                .map_err(|e| EngineError::QueryError(e.to_string()))?;
            tracing::info!(
                target: "metric_tree.explain",
                seq,
                database = %database,
                build_ms = build_start.elapsed().as_millis() as u64,
                "connector built (pool miss)"
            );
            (built, false)
        };

        let exec_start = std::time::Instant::now();
        let stream = handle
            .block_on(connector.execute_query_full(&sql))
            .map_err(|e| EngineError::QueryError(format!("{e:?}")))?;
        let exec_ms = exec_start.elapsed().as_millis() as u64;
        let decode_start = std::time::Instant::now();
        let rows = handle
            .block_on(crate::server::api::typed_stream::typed_stream_to_json_array(stream))
            .map_err(|e| EngineError::QueryError(format!("{e:?}")))?;
        let decode_ms = decode_start.elapsed().as_millis() as u64;
        let row_count = rows.len();
        let result = rows_to_maps(rows);
        result_cache
            .lock()
            .unwrap()
            .insert(result_key, result.clone());

        // Return the connector to the pool so the next parallel call can reuse it.
        pool.lock()
            .unwrap()
            .entry(database.clone())
            .or_default()
            .push(connector);

        tracing::info!(
            target: "metric_tree.explain",
            seq,
            database = %database,
            pool_hit,
            measures = ?request.measures,
            dimensions = ?request.dimensions,
            row_count,
            exec_ms,
            decode_ms,
            total_ms = total_start.elapsed().as_millis() as u64,
            sql_len = sql.len(),
            "query"
        );
        Ok(result)
    })
}

/// Resolve which configured database a query targets, from the datasource of
/// the first measure's view, falling back to the first workspace database.
fn resolve_database(
    engine: &oxy_airlayer_compat::SemanticEngine,
    request: &QueryRequest,
    databases: &[DatabaseConfig],
) -> Result<String, EngineError> {
    let member = request
        .measures
        .first()
        .or_else(|| request.dimensions.first())
        .ok_or_else(|| EngineError::QueryError("query has no measures".to_string()))?;
    let view_name = member.split('.').next().unwrap_or(member.as_str());
    // One rule with `predict_dialect`, so the dialect a predict judges these
    // rows by is the dialect of the warehouse that produced them.
    let datasource = engine.view(view_name).and_then(|v| v.datasource.clone());
    oxy_airlayer_compat::view_database(datasource.as_deref(), databases)
        .ok_or_else(|| EngineError::QueryError("no database configured".to_string()))
}

/// Convert a connector JSON response (`[header, ..rows]`, all-string cells)
/// into the column-keyed row maps airlayer's metric-tree ops expect. Numeric
/// cells are parsed back to JSON numbers so delta extraction works.
fn rows_to_maps(rows: Vec<Vec<String>>) -> Vec<Map<String, Value>> {
    let mut iter = rows.into_iter();
    let header = iter.next().unwrap_or_default();
    iter.map(|row| {
        header
            .iter()
            .zip(row)
            .map(|(key, cell)| {
                let value = if cell.is_empty() {
                    Value::Null
                } else if let Ok(n) = cell.parse::<f64>() {
                    serde_json::Number::from_f64(n)
                        .map(Value::Number)
                        .unwrap_or(Value::String(cell))
                } else {
                    Value::String(cell)
                };
                (key.clone(), value)
            })
            .collect()
    })
    .collect()
}

/// Where a metric-tree runner parses the semantic model from.
///
/// The three answers are not "a path or not": they are three different callers.
/// The IDE reads what is in front of the user; the stateless fleet reads what
/// the compile boundary handed it; workspace health reads the promoted revision
/// or nothing, because its job is to judge the workspace and the working copy
/// is whatever someone is editing right now.
#[derive(Debug, Clone)]
pub enum ScanRoot {
    /// Whatever this node has on disk. The IDE, local mode, and any caller that
    /// resolved nothing — reading the editable copy is the point.
    WorkingCopy,
    /// A scan the caller resolved through the compile boundary.
    Compiled(ScanDir),
    /// The caller requires the compile boundary and none was available. Falling
    /// back would answer with files the caller has said it must not judge, so
    /// answer with nothing instead.
    Unavailable,
}

impl ScanRoot {
    /// Why `effective_scan_path` came back empty, worded for whoever reads the
    /// failure — an operator in a health alert, a developer in a 500.
    fn unavailable_reason(&self) -> &'static str {
        match self {
            Self::Unavailable => {
                "the semantic model is not compiled for this workspace — this check reads the \
                 promoted revision only and will not fall back to a working copy, which may \
                 hold uncommitted edits"
            }
            _ => {
                "no semantic scan root on this node — resolve one through the compile boundary \
                 (with_scan) or run where the working copy lives"
            }
        }
    }
}

/// Construct an `Arc<dyn MetricTreeRunner>` from a workspace + user + role
/// triple. Used by [`crate::agentic_wiring::OxyProjectContext::metric_tree_runner`]
/// to populate the agentic platform port.
pub fn make_runner(
    workspace_manager: WorkspaceManager<WorkingCopy>,
    user_id: Uuid,
    role: WorkspaceRole,
    preagg: RunnerPreagg,
    scan_root: ScanRoot,
    holds_writes: bool,
) -> Arc<dyn MetricTreeRunner> {
    // The runner only needs the read capability; `into_read_only` keeps the
    // working copy, so the FS scan-path fallback still works on this node.
    let runner = OxyMetricTreeRunner::new(workspace_manager.into_read_only(), user_id, role)
        .with_preagg_ctx(preagg)
        .holding_writes(holds_writes);
    let runner = match scan_root {
        ScanRoot::Compiled(scan) => runner.with_scan(scan),
        ScanRoot::Unavailable => runner.without_working_copy_fallback(),
        ScanRoot::WorkingCopy => runner,
    };
    Arc::new(runner)
}

/// Strip dimensions from every view that are bad split candidates for
/// airlayer's `explain`. Drops:
///
/// - **All `Number`-typed dimensions.** Most numeric dims in real schemas
///   are either foreign keys (high cardinality → one breakdown row per
///   entity → minutes-long queries on warehouses without bucketing) or
///   continuous values (price, amount) that need bucketing to be
///   meaningful split candidates. Either way, surfacing them as raw
///   GROUP BY columns produces noisy results at high cost.
/// - **`exclude_members` and the `time_dimension`.** The monitor's
///   `group_by` / segment members are pinned by the base filter, so
///   splitting on them yields a single value (no signal); the time
///   dimension is the axis being compared, not a driver.
/// - **Time-derived dims** (`day_of_week`, `month`, `week`, …). Comparing
///   the same phase one seasonal cycle back, these are constant across the
///   two periods — attributing the change to "it's a Wednesday" is the
///   seasonality artifact this pruning exists to suppress.
/// - **High-cardinality row-key dims** (`*_key`, `*_id`, `id`). A per-row
///   key (e.g. `labor_day_key`) never matches across periods, so it always
///   "explains" 100% trivially.
///
/// Remaining strings + booleans (department, cost category, shift …) pass
/// through. The metric tree is rebuilt against the pruned layer so component
/// / driver edges stay intact.
///
/// The **key** half is declaration-driven as of 2026-08: airlayer's
/// `entities:` says which dimensions are row keys (`EntityType::Primary`) and
/// which are foreign keys, so a `restaurant_id` now survives as a split
/// candidate. The suffix heuristic remains only as a fallback for views that
/// declare no entities at all.
///
/// The **numeric** and **cardinality** halves are still a workaround until
/// airlayer exposes per-dim cardinality / `splittable` metadata and a
/// base-filter hook.
fn prune_dims_for_explain(
    mut layer: SemanticLayer,
    exclude_members: &[String],
    time_dimension: &str,
) -> SemanticLayer {
    use oxy_airlayer_compat::schema::models::DimensionType;
    for view in &mut layer.views {
        let view_name = view.name.clone();
        // Read the entity declaration out before `retain` borrows `view`
        // mutably — the row-key rule needs it and cannot reach `view` inside
        // the closure.
        let row_keys: Vec<String> = view
            .primary_key_dimensions()
            .into_iter()
            .map(str::to_string)
            .collect();
        let foreign_keys = foreign_key_dimensions(view);
        // Collect what we drop so a "why isn't dimension X in the
        // decomposition?" question is answerable from logs rather than by
        // re-deriving the heuristic. The pruning is otherwise silent.
        let mut dropped: Vec<&'static str> = Vec::new();
        let mut dropped_names: Vec<String> = Vec::new();
        view.dimensions.retain(|d| {
            let reason = if matches!(d.dimension_type, DimensionType::Number) {
                Some("numeric")
            } else {
                let fq = format!("{view_name}.{}", d.name);
                if fq == time_dimension {
                    Some("time_dimension")
                } else if exclude_members.iter().any(|m| m == &fq) {
                    Some("segment_filter")
                } else if is_seasonal_or_key_dim(&d.name) {
                    Some("time_part")
                } else if is_row_key_dim(&row_keys, &foreign_keys, d) {
                    Some("row_key")
                } else {
                    None
                }
            };
            match reason {
                Some(r) => {
                    dropped.push(r);
                    dropped_names.push(d.name.clone());
                    false
                }
                None => true,
            }
        });
        if !dropped_names.is_empty() {
            tracing::debug!(
                target: "metric_tree.explain",
                view = %view_name,
                dropped = ?dropped_names,
                reasons = ?dropped,
                "pruned dimensions from explain candidates"
            );
        }
    }
    layer
}

/// True for a dimension that identifies a *row* rather than a joinable entity.
///
/// The distinction the old `_id`/`_key` suffix rule could not make: a row key
/// (`sales_day_key`) never matches across periods and is a useless split
/// candidate; a foreign key (`restaurant_id`) is a legitimate grouping
/// dimension, and dropping it is why explain could never decompose a
/// chain-level anomaly by store.
///
/// airlayer declares this — `EntityType::Primary` owns the key,
/// `EntityType::Foreign` references another view's, and
/// `View::primary_key_dimensions()` returns exactly the former.
///
/// The fallback is **per dimension, not per view**. A view can declare only its
/// *foreign* entities — the common shape for a join target — so its
/// `primary_key_dimensions()` is empty while it still has row keys
/// (`sales_day_key`) the declaration never names. An all-or-nothing "declared →
/// trust only the declaration" rule lets those row keys survive pruning as
/// split candidates with cardinality equal to the row count, the exact failure
/// the pruning exists to prevent. So: prune a declared primary key, keep a
/// declared foreign key (a legit grouping dimension — dropping `restaurant_id`
/// is why explain could never decompose a chain-level anomaly by store), and
/// fall back to the suffix heuristic for anything the declaration doesn't
/// mention.
///
/// Takes the pre-computed key lists rather than `&View` because the only caller
/// runs inside `view.dimensions.retain(..)`, which already holds `view`
/// mutably.
fn is_row_key_dim(
    row_keys: &[String],
    foreign_keys: &[String],
    dim: &oxy_airlayer_compat::schema::models::Dimension,
) -> bool {
    if dim.primary_key == Some(true) || row_keys.iter().any(|k| k == &dim.name) {
        return true;
    }
    if foreign_keys.iter().any(|k| k == &dim.name) {
        return false;
    }
    has_key_suffix(&dim.name)
}

/// Declared foreign-key dimension names — the mirror of
/// [`oxy_airlayer_compat::schema::models::View::primary_key_dimensions`], which airlayer
/// does not expose for foreign entities. A foreign key references another
/// view's row, so it is a legitimate grouping dimension, not a row key.
fn foreign_key_dimensions(view: &oxy_airlayer_compat::schema::models::View) -> Vec<String> {
    use oxy_airlayer_compat::schema::models::EntityType;
    let mut fks: Vec<String> = Vec::new();
    for entity in &view.entities {
        if entity.entity_type != EntityType::Foreign {
            continue;
        }
        // An entity written as just `- name: restaurant_id` / `type: foreign`
        // declares no explicit `key:`/`keys:`, so `get_keys()` is empty; fall
        // back to its name. Without this it would contribute no foreign key, so
        // `is_row_key_dim` would reach `has_key_suffix("restaurant_id") == true`
        // and *prune* a legitimate split candidate — finding 6 in the opposite
        // direction (over-pruning rather than under-pruning).
        let keys = if entity.get_keys().is_empty() {
            vec![entity.name.clone()]
        } else {
            entity.get_keys()
        };
        for key in keys {
            if view.dimensions.iter().any(|d| d.name == key) {
                fks.push(key);
            }
        }
    }
    // Tidiness, not a correctness fix — the only consumer is an `any()`. But
    // `Vec::dedup` collapses only *adjacent* duplicates, so sort first if the
    // deduped list is ever exposed to a caller that cares about it.
    fks.sort();
    fks.dedup();
    fks
}

/// The pre-declaration heuristic: anything that *looks* like a key.
///
/// Kept only for unmodelled views. It cannot tell a row key from a foreign
/// key, which is the whole reason [`is_row_key_dim`] prefers the declaration.
fn has_key_suffix(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n == "id" || n == "key" || n.ends_with("_id") || n.ends_with("_key")
}

/// True for calendar-part dimensions — constant across a same-phase
/// comparison, so they always "explain" the difference trivially.
///
/// Key dimensions are no longer this function's business; see
/// [`is_row_key_dim`].
fn is_seasonal_or_key_dim(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const TIME_PARTS: &[&str] = &[
        "day_of_week",
        "dayofweek",
        "dow",
        "weekday",
        "day_of_month",
        "day_of_year",
        "week_of_year",
        "day",
        "week",
        "month",
        "quarter",
        "year",
        "hour",
        "minute",
        "second",
        "date",
        "datetime",
        "timestamp",
    ];
    TIME_PARTS.contains(&n.as_str())
}

/// Convert a pre-aggregation DuckDB result to the column-keyed row maps that
/// airlayer's metric-tree ops expect.
///
/// Returns `Err` only when the Parquet file is missing or DuckDB fails;
/// callers log the error and fall back to the warehouse path.
/// Convert a pre-aggregation DuckDB result to the column-keyed row maps that
/// airlayer's metric-tree ops expect.
///
/// `execute_preagg_sql` returns `{ "columns": [...], "rows": [...], ... }`.
/// Returns `Err` only when the Parquet file is missing or DuckDB fails;
/// callers log the error and fall back to the warehouse path.
fn execute_preagg_and_convert(
    preagg_sql: &str,
    source: &agentic_semantic::compile::PreaggSource,
) -> Result<Vec<Map<String, Value>>, String> {
    let json = agentic_semantic::preagg::execute_preagg_sql(preagg_sql, source)
        .map_err(|e| e.to_string())?;
    let rows = json
        .get("rows")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(rows
        .into_iter()
        .filter_map(|row| match row {
            Value::Object(m) => Some(m),
            _ => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn time_part_dims_are_pruned_regardless_of_entities() {
        // Calendar-part dims (constant across a same-phase comparison).
        for d in [
            "day_of_week",
            "DayOfWeek",
            "month",
            "week",
            "quarter",
            "date",
        ] {
            assert!(is_seasonal_or_key_dim(d), "{d} should be pruned");
        }
        // Real business dims survive. Key dims are no longer this rule's
        // business — see `is_row_key_dim`.
        for d in [
            "department",
            "cost_category",
            "shift",
            "region",
            "menu_category",
            "restaurant_id",
        ] {
            assert!(!is_seasonal_or_key_dim(d), "{d} should be kept");
        }
    }

    // `View`, `Dimension` and `Entity` have no `Default`, and hand-listing
    // ~20 optional fields would go stale on every airlayer bump. Deserializing
    // is both shorter and exactly how these arrive in production.
    fn test_dim(name: &str) -> oxy_airlayer_compat::schema::models::Dimension {
        serde_json::from_value(json!({ "name": name, "type": "string", "expr": name }))
            .expect("dimension fixture")
    }

    /// A view declaring `primary` as `EntityType::Primary` keys and `foreign`
    /// as `EntityType::Foreign` ones. Both named dimensions always exist.
    fn view_with_entities(
        primary: &[&str],
        foreign: &[&str],
    ) -> oxy_airlayer_compat::schema::models::View {
        let entity = |name: &str, kind: &str| json!({ "name": name, "type": kind, "key": name });
        let entities: Vec<_> = primary
            .iter()
            .map(|n| entity(n, "primary"))
            .chain(foreign.iter().map(|n| entity(n, "foreign")))
            .collect();
        serde_json::from_value(json!({
            "name": "sales_daily",
            "dimensions": [
                { "name": "sales_day_key", "type": "string", "expr": "sales_day_key" },
                { "name": "restaurant_id", "type": "string", "expr": "restaurant_id" },
            ],
            "entities": entities,
        }))
        .expect("view fixture")
    }

    /// The key lists `prune_dims_for_explain` extracts before its `retain`:
    /// `(primary/row keys, foreign keys)`.
    fn row_key_facts(
        view: &oxy_airlayer_compat::schema::models::View,
    ) -> (Vec<String>, Vec<String>) {
        (
            view.primary_key_dimensions()
                .into_iter()
                .map(str::to_string)
                .collect(),
            foreign_key_dimensions(view),
        )
    }

    #[test]
    fn a_declared_foreign_key_survives_pruning() {
        // restaurant_id is a join key and a legitimate grouping dimension.
        // Dropping it is why explain can never decompose a chain-level
        // anomaly by store — the most useful decomposition on offer.
        let view = view_with_entities(&["sales_day_key"], &["restaurant_id"]);
        let (row_keys, fks) = row_key_facts(&view);
        assert!(!is_row_key_dim(&row_keys, &fks, &test_dim("restaurant_id")));
        assert!(is_row_key_dim(&row_keys, &fks, &test_dim("sales_day_key")));
    }

    #[test]
    fn a_view_declaring_only_foreign_entities_still_prunes_its_row_keys() {
        // The regression the per-dimension fallback exists for: a fact view
        // that declares only its foreign entities has entities_declared == true
        // and primary_key_dimensions() == [], so an all-or-nothing rule would
        // return sales_day_key as a split candidate with cardinality equal to
        // the row count. The suffix fallback must still catch it, while the
        // declared foreign key restaurant_id stays a legitimate grouping dim.
        let view = view_with_entities(&[], &["restaurant_id"]);
        let (row_keys, fks) = row_key_facts(&view);
        assert!(
            is_row_key_dim(&row_keys, &fks, &test_dim("sales_day_key")),
            "a row key must be pruned even when the view declares only foreign entities"
        );
        assert!(
            !is_row_key_dim(&row_keys, &fks, &test_dim("restaurant_id")),
            "a declared foreign key is a split candidate, not a row key"
        );
    }

    #[test]
    fn a_keyless_foreign_entity_still_counts_as_a_split_candidate() {
        // A foreign entity written as just `- name: restaurant_id` / `type:
        // foreign`, no explicit `key:`. `get_keys()` is empty, so without the
        // name fallback restaurant_id would fall to has_key_suffix and be
        // pruned — over-pruning a legitimate split candidate.
        let view: oxy_airlayer_compat::schema::models::View = serde_json::from_value(json!({
            "name": "sales_daily",
            "dimensions": [
                { "name": "sales_day_key", "type": "string", "expr": "sales_day_key" },
                { "name": "restaurant_id", "type": "string", "expr": "restaurant_id" },
            ],
            "entities": [{ "name": "restaurant_id", "type": "foreign" }],
        }))
        .expect("view fixture");
        let (row_keys, fks) = row_key_facts(&view);
        assert_eq!(fks, vec!["restaurant_id".to_string()]);
        assert!(!is_row_key_dim(&row_keys, &fks, &test_dim("restaurant_id")));
        assert!(is_row_key_dim(&row_keys, &fks, &test_dim("sales_day_key")));
    }

    #[test]
    fn foreign_keys_are_deduped_across_entities() {
        // The same key named by two foreign entities must appear once.
        let view: oxy_airlayer_compat::schema::models::View = serde_json::from_value(json!({
            "name": "sales_daily",
            "dimensions": [{ "name": "restaurant_id", "type": "string", "expr": "restaurant_id" }],
            "entities": [
                { "name": "a", "type": "foreign", "key": "restaurant_id" },
                { "name": "b", "type": "foreign", "key": "restaurant_id" },
            ],
        }))
        .expect("view fixture");
        assert_eq!(
            foreign_key_dimensions(&view),
            vec!["restaurant_id".to_string()]
        );
    }

    #[test]
    fn an_unmodelled_view_falls_back_to_the_suffix_rule() {
        // A view with no `entities:` yields an empty primary_key_dimensions(),
        // so a declaration-only rule would prune nothing and return
        // sales_day_key as a split candidate. Today's heuristic is the right
        // answer there.
        let view = view_with_entities(&[], &[]);
        let (row_keys, fks) = row_key_facts(&view);
        assert!(is_row_key_dim(&row_keys, &fks, &test_dim("sales_day_key")));
        assert!(is_row_key_dim(&row_keys, &fks, &test_dim("restaurant_id")));
        assert!(!is_row_key_dim(&row_keys, &fks, &test_dim("department")));
    }

    /// `primary_key: true` on the dimension outranks the entity declaration —
    /// a view can mark a row key without modelling an entity for it.
    #[test]
    fn an_explicit_primary_key_flag_is_a_row_key() {
        let view = view_with_entities(&["sales_day_key"], &["restaurant_id"]);
        let (row_keys, fks) = row_key_facts(&view);
        let mut flagged = test_dim("restaurant_id");
        flagged.primary_key = Some(true);
        assert!(is_row_key_dim(&row_keys, &fks, &flagged));
    }

    #[test]
    fn execute_preagg_and_convert_returns_maps() {
        let dir = tempfile::tempdir().expect("tmpdir");
        let parquet_path = dir.path().join("test.parquet");

        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "COPY (SELECT 'foo' AS region, 42.0 AS revenue) TO '{}' (FORMAT PARQUET);",
            parquet_path.display()
        ))
        .unwrap();

        let preagg_sql = format!(
            "SELECT region, revenue FROM read_parquet('{}')",
            parquet_path.display()
        );

        let rows = execute_preagg_and_convert(
            &preagg_sql,
            &agentic_semantic::compile::PreaggSource::Local(parquet_path.clone()),
        )
        .expect("preagg convert should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get("region"), Some(&json!("foo")));
        let rev = rows[0].get("revenue").and_then(|v| v.as_f64());
        assert_eq!(rev, Some(42.0));
    }

    #[test]
    fn execute_preagg_and_convert_missing_file_returns_err() {
        let result = execute_preagg_and_convert(
            "SELECT 1",
            &agentic_semantic::compile::PreaggSource::Local(std::path::PathBuf::from(
                "/nonexistent/path.parquet",
            )),
        );
        assert!(result.is_err(), "missing Parquet should return Err");
    }

    #[test]
    fn default_timezone_reads_the_file_level_monitor_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".monitor.yml"),
            "timezone: America/Los_Angeles\nmonitors:\n  - measure: a.b\n    time_dimension: a.t\n",
        )
        .unwrap();
        assert_eq!(
            read_default_timezone(dir.path()),
            Some("America/Los_Angeles".to_string())
        );
    }

    #[test]
    fn default_timezone_is_none_without_a_monitor_config() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_default_timezone(dir.path()), None);
    }

    #[test]
    fn default_timezone_is_none_when_the_config_is_unreadable() {
        // A malformed .monitor.yml must not poison every metric-tree query —
        // fall back to UTC and let the scan path surface the real error.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(".monitor.yml"),
            "timezone: [not, a, string]\n",
        )
        .unwrap();
        assert_eq!(read_default_timezone(dir.path()), None);
    }

    #[test]
    fn non_utc_timezone_bypasses_preagg() {
        // Rollups are built UTC-truncated and preagg's match predicate ignores
        // timezone entirely, so a tz'd monitor served from a rollup would get
        // silently UTC-bucketed data. `for_timezone` must withhold the cache.
        let preagg = RunnerPreagg {
            cache: Some(Arc::new(RwLock::new(RefreshKeyCache::default()))),
            renewal_threshold_secs: 120,
            freshness: RollupFreshness::ServeStale,
        };

        assert!(
            preagg.for_timezone(None).cache.is_some(),
            "no timezone -> rollups still serve"
        );
        assert!(
            preagg.for_timezone(Some("UTC")).cache.is_some(),
            "explicit UTC -> rollups still serve"
        );
        assert!(
            preagg
                .for_timezone(Some("America/Los_Angeles"))
                .cache
                .is_none(),
            "non-UTC -> must bypass the rollup path"
        );
        assert_eq!(
            preagg
                .for_timezone(Some("America/Los_Angeles"))
                .renewal_threshold_secs,
            120,
            "withholding the cache must not disturb the rest of the posture"
        );
    }
}
