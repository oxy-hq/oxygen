//! `ReconcileRunner`: runs every check in a workspace's `reconcile.yml` and
//! returns drift verdicts. Each check compares two operands (`actual` /
//! `expected`), each one of `semantic` / `sql` / `external` / `constant`. The
//! runner collects every external operand across both slots of all checks and
//! batch-fetches per `(source, integration)` (one Toast report per window,
//! shared), evaluates the non-external operands directly, then pairs each
//! check's two scalars into a verdict. A check with `group_by` also evaluates
//! both operands per group and attaches the disputed segments to its verdict.
//! The sweep depends only on the source trait, so it is testable with a fake.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use agentic_analytics::MetricTreeRunner;
use agentic_pipeline::platform::ProjectContext;
use async_trait::async_trait;
use sea_orm::DatabaseConnection;

use oxy::config::model::ToastAnalyticsIntegration;
use oxy::service::secret_manager::SecretManagerService;
use oxy_app::surface::workspace_runtime::{OxyProjectContext, build_workspace_ctx};
use oxy_workspace_health::reconcile::config::{
    ExternalKey, ExternalSpec, GroupByPlan, Operand, OperandKind, ReconcileCheck,
    parse_reconcile_config,
};
use oxy_workspace_health::reconcile::oxy_query::{
    cell_to_f64, grouped_rows, grouped_semantic_request, render_sql, semantic_request,
};
use oxy_workspace_health::reconcile::segments::{GroupedSide, fan_out, segment_error};
use oxy_workspace_health::reconcile::source::{
    ExternalRequest, ExternalValue, ReconcileError, SourceCtx, source_for,
};
use oxy_workspace_health::reconcile::window::resolve_window;
use oxy_workspace_health::reconcile::{
    DriftVerdict, ReconcileRunner, ResolvedWindow, VerdictMeta, compare, error_verdict,
    unreachable_verdict,
};

/// Which slot of a check an operand fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Side {
    Actual,
    Expected,
}

pub struct LiveReconcileRunner {
    now: chrono::DateTime<chrono::Utc>,
    /// Whole-report time budget for an async external report (create + poll).
    report_timeout: Duration,
    pct_unhealthy: f64,
    /// Central Postgres handle, needed to build the per-workspace semantic
    /// context that runs a semantic operand. `None` (e.g. in unit tests) leaves
    /// the measure unwired — such operands surface as an error.
    db: Option<DatabaseConnection>,
}

/// An external operand awaiting a fetch, tagged by its slot and carrying the
/// already-resolved window. `spec` borrows the check; `window` is owned so the
/// `ExternalRequest` can borrow it during the batch.
struct ExternalSlot<'a> {
    check_index: usize,
    side: Side,
    spec: &'a ExternalSpec,
    window: [String; 2],
    /// The check's `group_by` in source dimensions; empty when ungrouped.
    group_by: Vec<ExternalKey>,
}

/// Row cap for a grouped `sql` operand. A result that hits it is refused rather
/// than summed short, because the total it feeds is the verdict's number.
const MAX_GROUPED_SQL_ROWS: u64 = 10_000;

/// One operand of a `group_by` check: the total the verdict compares, and the
/// per-group values its segments are cut from. `groups` failing leaves the
/// verdict intact and only marks the detail unavailable.
struct GroupedOperand {
    total: f64,
    groups: Result<GroupedSide, String>,
}

/// How an operand evaluation failed, mapped to the matching degraded verdict.
enum OperandFailure {
    /// External source unreachable — Degraded, never Unhealthy.
    Unreachable(String),
    /// Any other pre-comparison failure (bad measure, missing secret, unknown
    /// source, validation error).
    Error(String),
}

impl OperandFailure {
    fn into_verdict(self, meta: &VerdictMeta) -> DriftVerdict {
        match self {
            OperandFailure::Unreachable(source) => unreachable_verdict(meta, &source),
            OperandFailure::Error(msg) => error_verdict(meta, msg),
        }
    }
}

impl LiveReconcileRunner {
    pub fn from_env(now: chrono::DateTime<chrono::Utc>) -> Self {
        let report_timeout = std::env::var("OXY_RECONCILE_REPORT_TIMEOUT_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        let pct_unhealthy = std::env::var("OXY_RECONCILE_PCT_UNHEALTHY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5.0);
        Self {
            now,
            report_timeout: Duration::from_secs(report_timeout),
            pct_unhealthy,
            db: None,
        }
    }

    /// Attach the DB handle so semantic/SQL operands can execute (the sweep
    /// builds a workspace-scoped semantic context from it).
    pub fn with_db(mut self, db: DatabaseConnection) -> Self {
        self.db = Some(db);
        self
    }

    /// Build the SYSTEM-mode metric-tree runner for this workspace once, reused
    /// across all of its checks. `None` when the runner can't be built —
    /// semantic operands then surface as an error, the safe default.
    fn build_measure_runner(ctx: &OxyProjectContext) -> Option<Arc<dyn MetricTreeRunner>> {
        ctx.metric_tree_runner_system()
    }

    /// Resolve every `toast_analytics` integration in `config.yml` (paired with
    /// its name), so an operand can bind to a specific account by name. Empty
    /// when none are declared.
    fn resolve_toast_integrations(
        ctx: &OxyProjectContext,
    ) -> Vec<(String, ToastAnalyticsIntegration)> {
        ctx.workspace_manager()
            .config_manager
            .toast_analytics_integrations()
            .into_iter()
            .map(|(name, t)| (name.to_string(), t.clone()))
            .collect()
    }

    /// Collect every external operand across both slots of all checks.
    /// `windows[i]` is the resolved (calendar-snapped) window for check `i`.
    fn collect_external_slots<'a>(
        checks: &'a [ReconcileCheck],
        windows: &[ResolvedWindow],
    ) -> Vec<ExternalSlot<'a>> {
        let mut slots = Vec::new();
        for (i, check) in checks.iter().enumerate() {
            for (side, operand) in [
                (Side::Actual, &check.actual),
                (Side::Expected, &check.expected),
            ] {
                if let Ok(OperandKind::External(spec)) = operand.resolve_kind() {
                    slots.push(ExternalSlot {
                        check_index: i,
                        side,
                        spec,
                        window: windows[i].dates.clone(),
                        // Validated at parse; an unmapped key cannot reach here.
                        group_by: spec.group_keys(&check.group_keys()).unwrap_or_default(),
                    });
                }
            }
        }
        slots
    }

    /// Batch-fetch every external operand, grouped by `(source, integration)` so
    /// each distinct external account fetches its report once (shared across all
    /// external operands in the group, either side, all checks). Result keyed by
    /// `(check_index, Side)`.
    async fn fetch_all_externals(
        &self,
        workspace_id: uuid::Uuid,
        checks: &[ReconcileCheck],
        windows: &[ResolvedWindow],
        toast_integrations: &[(String, ToastAnalyticsIntegration)],
    ) -> HashMap<(usize, Side), Result<ExternalValue, ReconcileError>> {
        let slots = Self::collect_external_slots(checks, windows);
        let mut by_group: BTreeMap<(&str, Option<&str>), Vec<usize>> = BTreeMap::new();
        for (slot_idx, slot) in slots.iter().enumerate() {
            by_group
                .entry((slot.spec.source.as_str(), slot.spec.integration.as_deref()))
                .or_default()
                .push(slot_idx);
        }

        let mut out = HashMap::new();
        for ((source_id, integration_name), slot_idxs) in by_group {
            let toast = pick_toast(toast_integrations, integration_name);
            let requests: Vec<ExternalRequest<'_>> = slot_idxs
                .iter()
                .map(|&si| ExternalRequest {
                    spec: slots[si].spec,
                    window: &slots[si].window,
                    group_by: &slots[si].group_by,
                })
                .collect();
            let results = self
                .fetch_group(workspace_id, source_id, &requests, toast)
                .await;
            for (&si, r) in slot_idxs.iter().zip(results) {
                let slot = &slots[si];
                out.insert((slot.check_index, slot.side), r);
            }
        }
        out
    }

    /// Resolve one source's secrets and delegate to its batched fetch. Unknown
    /// source → an `Unknown` error per request. `toast` carries the workspace's
    /// resolved integration (secret var-names + base URL); `None` lets the
    /// source fall back to its built-in defaults.
    async fn fetch_group(
        &self,
        workspace_id: uuid::Uuid,
        source_id: &str,
        requests: &[ExternalRequest<'_>],
        toast: Option<&ToastAnalyticsIntegration>,
    ) -> Vec<Result<ExternalValue, ReconcileError>> {
        let Some(source) = source_for(source_id, toast) else {
            return requests
                .iter()
                .map(|_| Err(ReconcileError::Unknown(source_id.to_string())))
                .collect();
        };
        // `SecretManagerService`'s `project_id` parameter IS the workspace id in
        // Oxy — there is no separate projects table, and every secret read/write
        // site keys the manager by `workspace_id`. Passing it here matches how
        // the secrets were stored.
        let secret_manager = SecretManagerService::new(workspace_id);
        let mut secrets = HashMap::new();
        // Resolve each configured var name, keying the value by its logical slot
        // so the source reads it back by slot regardless of the var name.
        for (slot, var) in source.secret_vars() {
            if let Some(value) = secret_manager.get_secret(&var).await {
                secrets.insert(slot.to_string(), value);
            }
        }
        let ctx = SourceCtx {
            now: self.now,
            secrets,
            report_timeout: self.report_timeout,
        };
        source.fetch_externals(&ctx, requests).await
    }

    /// Evaluate one check's two operands to scalars and compare. Every failure
    /// maps to a Degraded verdict (never a panic, never aborts the sweep).
    async fn verdict_for_check(
        &self,
        workspace_id: uuid::Uuid,
        check_index: usize,
        check: &ReconcileCheck,
        window: &ResolvedWindow,
        measure_runner: &Option<Arc<dyn MetricTreeRunner>>,
        externals: &HashMap<(usize, Side), Result<ExternalValue, ReconcileError>>,
    ) -> DriftVerdict {
        let meta = VerdictMeta {
            check: check.name.clone(),
            description: check.description.clone(),
            actual_label: check.actual.label_or("Actual"),
            expected_label: check.expected.label_or("Expected"),
            window: window.clone(),
        };
        match check.group_by_plan() {
            // A bad group_by degrades its own check, never the file.
            Err(reason) => error_verdict(&meta, reason),
            Ok(GroupByPlan::Grouped) => {
                self.grouped_verdict(
                    workspace_id,
                    check_index,
                    check,
                    &meta,
                    measure_runner,
                    externals,
                )
                .await
            }
            Ok(GroupByPlan::Ungrouped(note)) => {
                let mut verdict = self
                    .ungrouped_verdict(
                        workspace_id,
                        check_index,
                        check,
                        &meta,
                        measure_runner,
                        externals,
                    )
                    .await;
                verdict.segments = Some(segment_error(&check.group_keys(), note));
                verdict
            }
            Ok(GroupByPlan::None) => {
                self.ungrouped_verdict(
                    workspace_id,
                    check_index,
                    check,
                    &meta,
                    measure_runner,
                    externals,
                )
                .await
            }
        }
    }

    /// The comparison every check without `group_by` gets: each operand to a
    /// scalar, then `compare`.
    async fn ungrouped_verdict(
        &self,
        workspace_id: uuid::Uuid,
        check_index: usize,
        check: &ReconcileCheck,
        meta: &VerdictMeta,
        measure_runner: &Option<Arc<dyn MetricTreeRunner>>,
        externals: &HashMap<(usize, Side), Result<ExternalValue, ReconcileError>>,
    ) -> DriftVerdict {
        let window = &meta.window;
        let actual = match self
            .eval_operand(
                workspace_id,
                (check_index, Side::Actual),
                &check.actual,
                window,
                measure_runner,
                externals,
            )
            .await
        {
            Ok(v) => v,
            Err(f) => return f.into_verdict(meta),
        };
        let expected = match self
            .eval_operand(
                workspace_id,
                (check_index, Side::Expected),
                &check.expected,
                window,
                measure_runner,
                externals,
            )
            .await
        {
            Ok(v) => v,
            Err(f) => return f.into_verdict(meta),
        };
        compare(meta, actual, expected, &check.tolerance, self.pct_unhealthy)
    }

    /// A `group_by` check: the verdict is compared on the operands' totals, the
    /// same way an ungrouped check is, and the per-group comparison is attached
    /// as `segments`. A grouped `sql` operand's total is the sum of its rows (it
    /// returns one row per group, so there is no separate scalar to read).
    async fn grouped_verdict(
        &self,
        workspace_id: uuid::Uuid,
        check_index: usize,
        check: &ReconcileCheck,
        meta: &VerdictMeta,
        measure_runner: &Option<Arc<dyn MetricTreeRunner>>,
        externals: &HashMap<(usize, Side), Result<ExternalValue, ReconcileError>>,
    ) -> DriftVerdict {
        let mut sides = Vec::with_capacity(2);
        for (side, operand) in [
            (Side::Actual, &check.actual),
            (Side::Expected, &check.expected),
        ] {
            match self
                .eval_grouped_operand(
                    workspace_id,
                    (check_index, side),
                    operand,
                    check,
                    &meta.window,
                    measure_runner,
                    externals,
                )
                .await
            {
                Ok(g) => sides.push(g),
                Err(f) => return f.into_verdict(meta),
            }
        }
        let expected = sides.pop().expect("two sides");
        let actual = sides.pop().expect("two sides");
        let mut verdict = compare(
            meta,
            actual.total,
            expected.total,
            &check.tolerance,
            self.pct_unhealthy,
        );
        let keys = check.group_keys();
        verdict.segments = Some(match (actual.groups, expected.groups) {
            (Ok(a), Ok(e)) => fan_out(&keys, a, e, &check.tolerance),
            (Err(err), _) | (_, Err(err)) => segment_error(&keys, err),
        });
        verdict
    }

    /// Evaluate one operand of a `group_by` check to its total and its groups.
    async fn eval_grouped_operand(
        &self,
        workspace_id: uuid::Uuid,
        key: (usize, Side),
        operand: &Operand,
        check: &ReconcileCheck,
        window: &ResolvedWindow,
        measure_runner: &Option<Arc<dyn MetricTreeRunner>>,
        externals: &HashMap<(usize, Side), Result<ExternalValue, ReconcileError>>,
    ) -> Result<GroupedOperand, OperandFailure> {
        let kind = operand.resolve_kind().map_err(OperandFailure::Error)?;
        match kind {
            OperandKind::Constant(n) => Ok(GroupedOperand {
                total: n,
                groups: Ok(GroupedSide::Broadcast(n)),
            }),
            OperandKind::Sql(spec) => {
                let rows = self
                    .run_sql_grouped(
                        workspace_id,
                        &spec.database,
                        &spec.sql,
                        window,
                        &check.group_keys(),
                    )
                    .await
                    .map_err(OperandFailure::Error)?;
                Ok(GroupedOperand {
                    total: rows.iter().map(|(_, v)| v).sum(),
                    groups: Ok(GroupedSide::Rows(rows)),
                })
            }
            OperandKind::Semantic(spec) => {
                // The total is the ungrouped scalar, not a sum of the groups:
                // an average or count-distinct does not add up across groups.
                let total = self
                    .eval_operand(
                        workspace_id,
                        key,
                        operand,
                        window,
                        measure_runner,
                        externals,
                    )
                    .await?;
                let runner = measure_runner
                    .as_ref()
                    .ok_or_else(|| OperandFailure::Error("no workspace semantic context".into()))?;
                let (request, keys) = grouped_semantic_request(
                    &spec.query,
                    &spec.time_dimension,
                    window,
                    &spec.group_dimensions(&check.group_by),
                );
                let groups = runner
                    .run_query_grouped(request, keys)
                    .await
                    .map(GroupedSide::Rows)
                    .map_err(|e| e.to_string());
                Ok(GroupedOperand { total, groups })
            }
            // Cut from the same pre-fetched report as the total.
            OperandKind::External(spec) => {
                let total = self
                    .eval_operand(
                        workspace_id,
                        key,
                        operand,
                        window,
                        measure_runner,
                        externals,
                    )
                    .await?;
                let groups = match externals.get(&key) {
                    Some(Ok(ExternalValue {
                        groups: Some(groups),
                        ..
                    })) => Ok(GroupedSide::Rows(groups.clone())),
                    _ => Err(format!(
                        "source '{}' returned no per-group values",
                        spec.source
                    )),
                };
                Ok(GroupedOperand { total, groups })
            }
        }
    }

    /// Resolve one operand to a scalar per its kind. External operands read the
    /// pre-fetched `externals` map by `(check_index, Side)`; the others execute
    /// directly. Any failure surfaces as an `OperandFailure` so the check
    /// degrades safely.
    async fn eval_operand(
        &self,
        workspace_id: uuid::Uuid,
        key: (usize, Side),
        operand: &Operand,
        window: &ResolvedWindow,
        measure_runner: &Option<Arc<dyn MetricTreeRunner>>,
        externals: &HashMap<(usize, Side), Result<ExternalValue, ReconcileError>>,
    ) -> Result<f64, OperandFailure> {
        let kind = operand.resolve_kind().map_err(OperandFailure::Error)?;
        match kind {
            OperandKind::Constant(n) => Ok(n),
            OperandKind::Semantic(spec) => {
                let runner = measure_runner
                    .as_ref()
                    .ok_or_else(|| OperandFailure::Error("no workspace semantic context".into()))?;
                let request = semantic_request(&spec.query, &spec.time_dimension, window);
                runner
                    .run_query_scalar(request)
                    .await
                    .map_err(|e| OperandFailure::Error(e.to_string()))
            }
            OperandKind::Sql(spec) => self
                .run_sql(workspace_id, &spec.database, &spec.sql, window)
                .await
                .map_err(OperandFailure::Error),
            OperandKind::External(spec) => match externals.get(&key) {
                Some(Ok(v)) => Ok(v.total),
                Some(Err(ReconcileError::Unreachable(_))) => {
                    Err(OperandFailure::Unreachable(spec.source.clone()))
                }
                Some(Err(ReconcileError::Unknown(_))) => Err(OperandFailure::Error(format!(
                    "unknown reconcile source: {}",
                    spec.source
                ))),
                Some(Err(e)) => Err(OperandFailure::Error(e.to_string())),
                None => Err(OperandFailure::Error(
                    "external value not fetched".to_string(),
                )),
            },
        }
    }

    /// Run a raw-SQL operand against the named workspace connection, reducing
    /// the first cell of the first row to a scalar. `{{ start_date }}` /
    /// `{{ end_date }}` in the SQL are bound from the resolved window.
    async fn run_sql(
        &self,
        workspace_id: uuid::Uuid,
        database: &str,
        sql: &str,
        period: &ResolvedWindow,
    ) -> Result<f64, String> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| "no workspace context".to_string())?;
        let ctx = build_workspace_ctx(workspace_id, db)
            .await
            .ok_or_else(|| "no workspace context".to_string())?;
        // Use the general, all-warehouse connector builder (the same path the
        // semantic operand takes via the metric-tree runner) rather than the
        // airhouse-only `resolve_pre_built_connector`, which early-returns `None`
        // for ClickHouse/Snowflake/BigQuery/Postgres and collapses to a flat
        // "not configured" error. `build_connector_for` dispatches every
        // warehouse type and surfaces the real error.
        let connector = ctx
            .build_connector_for(database)
            .await
            .map_err(|e| format!("reconcile sql: database '{database}': {e}"))?;
        let rendered = render_sql(sql, period)?;
        let res = connector
            .execute_query(&rendered, 1)
            .await
            .map_err(|e| e.to_string())?;
        let row = res
            .result
            .rows
            .first()
            .ok_or_else(|| "reconcile sql returned no rows".to_string())?;
        let cell = row
            .0
            .first()
            .ok_or_else(|| "reconcile sql returned no columns".to_string())?;
        cell_to_f64(cell)
    }

    /// Run a grouped raw-SQL operand: one row per group, a column per
    /// `group_by` key plus one value column (see [`grouped_rows`]).
    async fn run_sql_grouped(
        &self,
        workspace_id: uuid::Uuid,
        database: &str,
        sql: &str,
        period: &ResolvedWindow,
        keys: &[String],
    ) -> Result<Vec<(Vec<String>, f64)>, String> {
        let db = self
            .db
            .as_ref()
            .ok_or_else(|| "no workspace context".to_string())?;
        let ctx = build_workspace_ctx(workspace_id, db)
            .await
            .ok_or_else(|| "no workspace context".to_string())?;
        let connector = ctx
            .build_connector_for(database)
            .await
            .map_err(|e| format!("reconcile sql: database '{database}': {e}"))?;
        let rendered = render_sql(sql, period)?;
        let res = connector
            .execute_query(&rendered, MAX_GROUPED_SQL_ROWS)
            .await
            .map_err(|e| e.to_string())?;
        if res.result.truncated {
            return Err(format!(
                "reconcile sql: grouped result has {} rows, over the {MAX_GROUPED_SQL_ROWS} \
                 a group_by check reads",
                res.result.total_row_count
            ));
        }
        grouped_rows(&res.result.columns, &res.result.rows, keys)
    }
}

/// Pick the toast integration an operand binds to: by `name` when it named one,
/// else the first declared (back-compat for single-account workspaces). `None`
/// when the named integration is absent or none exist — the source then
/// resolves no secrets and the check degrades to NotConfigured.
fn pick_toast<'a>(
    integrations: &'a [(String, ToastAnalyticsIntegration)],
    name: Option<&str>,
) -> Option<&'a ToastAnalyticsIntegration> {
    match name {
        Some(n) => integrations.iter().find(|(nm, _)| nm == n).map(|(_, t)| t),
        None => integrations.first().map(|(_, t)| t),
    }
}

#[async_trait]
impl ReconcileRunner for LiveReconcileRunner {
    async fn run_checks(&self, workspace_id: uuid::Uuid) -> Vec<DriftVerdict> {
        // One context for the whole pass, so the config, the measure runner and
        // the toast bindings all describe the same revision. Building it per
        // helper let a compile landing mid-run split a verdict across two.
        let Some(db) = self.db.as_ref() else {
            return Vec::new();
        };
        let Some(ctx) = build_workspace_ctx(workspace_id, db).await else {
            return Vec::new();
        };

        // Per the compile-boundary contract, readers fall through to the
        // filesystem on a miss — so a workspace with no compiled
        // `reconcile.yml` still reconciles from the working copy.
        let (raw, source) = match ctx
            .workspace_manager()
            .config_manager
            .reconcile_config()
            .await
        {
            Ok(Some(v)) => (
                v,
                match ctx.workspace_manager().config_manager.origin() {
                    oxy::config::Origin::Compiled { .. } => "compiled",
                    oxy::config::Origin::Disk => "fs",
                },
            ),
            Ok(None) => return Vec::new(),
            Err(e) => {
                tracing::warn!(target: "health_eval", %workspace_id, error = %e,
                    "reconcile config read failed");
                return Vec::new();
            }
        };
        let cfg = match parse_reconcile_config(&raw) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(target: "health_eval", %workspace_id, error = %e,
                    "reconcile config parse failed");
                return Vec::new();
            }
        };
        tracing::info!(
            target: "health_eval",
            %workspace_id,
            source,
            checks = cfg.checks.len(),
            "running reconcile checks"
        );

        // One workspace-scoped measure runner, reused across every check.
        let measure_runner = Self::build_measure_runner(&ctx);
        // Every toast integration the workspace declares, resolved once; each
        // external operand binds to the one it names.
        let toast_integrations = Self::resolve_toast_integrations(&ctx);

        // Resolve each check's window once (shared by both operands), then
        // batch-fetch every external operand across all checks.
        let windows: Vec<ResolvedWindow> = cfg
            .checks
            .iter()
            .map(|c| resolve_window(&c.window, self.now))
            .collect();
        for (check, w) in cfg.checks.iter().zip(&windows) {
            // The window is the single most load-bearing input to a drift
            // number, and `freshness` is invisible in the config as deployed
            // (it can be inherited from the file level). Log the resolved one
            // next to the freshness that produced it so an operator can tell a
            // real drift from a period the warehouse hasn't finished loading.
            //
            // Read through the same `effective_freshness()` accessor
            // `resolve_window` uses, so the logged watermark cannot drift from
            // the one that actually produced the window above.
            tracing::info!(
                target: "health_eval",
                %workspace_id,
                check = %check.name,
                window = format!("{}..{}", w.dates[0], w.dates[1]),
                timezone = %w.timezone,
                freshness = %humantime::format_duration(
                    check.window.effective_freshness().to_std().unwrap_or_default()
                ),
                "reconcile check window resolved"
            );
        }
        let externals = self
            .fetch_all_externals(workspace_id, &cfg.checks, &windows, &toast_integrations)
            .await;

        let mut out = Vec::with_capacity(cfg.checks.len());
        for (i, check) in cfg.checks.iter().enumerate() {
            out.push(
                self.verdict_for_check(
                    workspace_id,
                    i,
                    check,
                    &windows[i],
                    &measure_runner,
                    &externals,
                )
                .await,
            );
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use oxy_workspace_health::evaluator::HealthStatus;

    struct FakeReconcileRunner {
        verdicts: Vec<DriftVerdict>,
    }

    #[async_trait::async_trait]
    impl ReconcileRunner for FakeReconcileRunner {
        async fn run_checks(&self, _workspace_id: uuid::Uuid) -> Vec<DriftVerdict> {
            self.verdicts.clone()
        }
    }

    fn meta(check: &str) -> VerdictMeta {
        VerdictMeta {
            check: check.to_string(),
            description: None,
            actual_label: "Actual".to_string(),
            expected_label: "Expected".to_string(),
            window: ResolvedWindow {
                dates: ["2026-07-12".to_string(), "2026-07-18".to_string()],
                timezone: "UTC".to_string(),
            },
        }
    }

    #[tokio::test]
    async fn fake_runner_returns_canned_verdicts() {
        let r = FakeReconcileRunner {
            verdicts: vec![unreachable_verdict(&meta("c"), "toast")],
        };
        let out = r.run_checks(uuid::Uuid::nil()).await;
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].status, HealthStatus::Degraded);
    }

    fn toast_named(name: &str) -> (String, ToastAnalyticsIntegration) {
        (
            name.to_string(),
            ToastAnalyticsIntegration {
                client_id_var: Some(format!("{name}_ID")),
                client_secret_var: None,
                api_token_var: None,
                base_url: None,
            },
        )
    }

    #[test]
    fn pick_toast_binds_by_name_then_falls_back_to_first() {
        let list = vec![toast_named("main"), toast_named("second")];
        // Named match.
        assert_eq!(
            pick_toast(&list, Some("second")).unwrap().client_id_var,
            Some("second_ID".to_string())
        );
        // No name → first declared.
        assert_eq!(
            pick_toast(&list, None).unwrap().client_id_var,
            Some("main_ID".to_string())
        );
        // Named but absent → None (check degrades to NotConfigured).
        assert!(pick_toast(&list, Some("ghost")).is_none());
        // No integrations declared → None.
        assert!(pick_toast(&[], None).is_none());
    }

    #[test]
    fn env_defaults_are_sane() {
        let r = LiveReconcileRunner::from_env(chrono::Utc::now());
        assert_eq!(r.pct_unhealthy, 5.0);
        assert_eq!(r.report_timeout.as_secs(), 60);
    }

    #[test]
    fn sql_scalar_reduces_first_cell() {
        use agentic_core::result::{CellValue, QueryRow};
        use oxy_workspace_health::reconcile::oxy_query::cell_to_f64;
        // Shape a one-cell result like execute_query(sql, 1) returns; the runner
        // reduces res.result.rows[0].0[0] via cell_to_f64.
        let rows = [QueryRow(vec![CellValue::Number(1234.5)])];
        let cell = &rows.first().unwrap().0[0];
        assert_eq!(cell_to_f64(cell).unwrap(), 1234.5);

        // Empty result ⇒ the runner surfaces "no rows".
        let empty: Vec<QueryRow> = vec![];
        assert!(empty.first().is_none());
    }

    use oxy_workspace_health::reconcile::Tolerance;
    use oxy_workspace_health::reconcile::compare::Combinator;
    use oxy_workspace_health::reconcile::config::Grain;
    use oxy_workspace_health::reconcile::config::{Operand, SemanticSpec, Window};

    fn semantic_operand(measure: &str) -> Operand {
        let mut query = oxy_airlayer_compat::engine::query::QueryRequest::new();
        query.measures = vec![measure.to_string()];
        Operand {
            label: None,
            semantic: Some(SemanticSpec {
                query,
                time_dimension: "m.d".to_string(),
            }),
            sql: None,
            external: None,
            constant: None,
        }
    }

    fn constant_operand(n: f64) -> Operand {
        Operand {
            label: None,
            semantic: None,
            sql: None,
            external: None,
            constant: Some(n),
        }
    }

    fn check_with_grain(grain: Grain) -> ReconcileCheck {
        ReconcileCheck {
            name: "c".to_string(),
            description: None,
            window: Window {
                last: 1,
                grain,
                offset: 1,
                freshness: None,
                timezone: None,
                week_start: oxy_workspace_health::reconcile::WeekStart::Sunday,
            },
            tolerance: Tolerance {
                abs: 1.0,
                pct: 0.5,
                combinator: Combinator::And,
            },
            group_by: Vec::new(),
            actual: semantic_operand("m.x"),
            expected: constant_operand(100.0),
        }
    }

    #[tokio::test]
    async fn week_month_grain_evaluate_not_degrade() {
        // Week/month grains now snap to calendar boundaries and compare like any
        // other window — they must NOT degrade on grain alone.
        let r = LiveReconcileRunner::from_env(chrono::Utc::now());
        for grain in [Grain::Week, Grain::Month] {
            let mut check = check_with_grain(grain);
            // Both sides constant so the check evaluates with no runner/external.
            check.actual = constant_operand(5.0);
            check.expected = constant_operand(5.0);
            let window = resolve_window(&check.window, r.now);
            let v = r
                .verdict_for_check(
                    uuid::Uuid::nil(),
                    0,
                    &check,
                    &window,
                    &None,
                    &HashMap::new(),
                )
                .await;
            assert_eq!(v.status, HealthStatus::Healthy, "{grain:?} should compare");
        }
    }

    /// The exact shape the Toast reconcile ships: file-level `freshness`, weekly
    /// checks with `offset: 1`, one Oxy semantic operand vs one Toast external
    /// operand.
    fn weekly_toast_yaml(freshness: &str) -> serde_json::Value {
        let yaml = format!(
            r#"
freshness: {freshness}
checks:
  - name: net_sales_vs_toast_weekly
    window: {{ last: 1, grain: week, offset: 1 }}
    tolerance: {{ abs: 1.0, pct: 0.5 }}
    actual:
      label: Oxy net sales (wk)
      semantic:
        measures: [sales.net_sales]
        time_dimension: sales.business_date
    expected:
      label: Toast net sales (wk)
      external:
        source: toast
        metric: net_sales
"#
        );
        serde_yaml::from_str(&yaml).unwrap()
    }

    #[test]
    fn file_level_freshness_reaches_the_toast_request_window() {
        // The chain this pins: reconcile.yml `freshness` → apply_defaults →
        // Window.freshness → resolve_window → the [start, end] handed to the
        // Toast report. Each link had its own test; nothing covered them joined,
        // which is exactly where a freshness watermark can be silently dropped.
        //
        // Reference Wed 2026-07-29. Sunday-start current week began 2026-07-26,
        // so with NO freshness "last full week" is 2026-07-19..2026-07-25 — the
        // window whose final days a lagging warehouse has not loaded yet.
        let now = chrono::Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap();
        let cfg = parse_reconcile_config(&weekly_toast_yaml("0s")).unwrap();
        let windows: Vec<ResolvedWindow> = cfg
            .checks
            .iter()
            .map(|c| resolve_window(&c.window, now))
            .collect();
        assert_eq!(windows[0].dates, ["2026-07-19", "2026-07-25"]);

        // With `freshness: 6d` the reference lands on Thu 2026-07-23, whose week
        // began 2026-07-19 — so "last full week" steps back one whole week to a
        // period a 6-day-behind warehouse has fully loaded.
        let cfg = parse_reconcile_config(&weekly_toast_yaml("6d")).unwrap();
        // The check itself set no `freshness`; it must have inherited the
        // file-level one, or the window below silently would not move.
        assert_eq!(
            cfg.checks[0].window.effective_freshness(),
            chrono::Duration::days(6)
        );
        let windows: Vec<ResolvedWindow> = cfg
            .checks
            .iter()
            .map(|c| resolve_window(&c.window, now))
            .collect();
        assert_eq!(windows[0].dates, ["2026-07-12", "2026-07-18"]);

        // And that shifted window is what the Toast operand actually fetches —
        // both operands share it, so the two sides stay comparable.
        let slots = LiveReconcileRunner::collect_external_slots(&cfg.checks, &windows);
        assert_eq!(slots.len(), 1, "the toast operand must be collected");
        assert_eq!(slots[0].side, Side::Expected);
        assert_eq!(slots[0].window, ["2026-07-12", "2026-07-18"]);
    }

    #[test]
    fn freshness_shift_survives_to_the_rendered_verdict() {
        // A verdict that doesn't state its window makes `freshness` unfalsifiable
        // from the health payload — the admin Reconciliation tab shows the drift
        // but not the period it was measured over.
        let now = chrono::Utc.with_ymd_and_hms(2026, 7, 29, 12, 0, 0).unwrap();
        let cfg = parse_reconcile_config(&weekly_toast_yaml("6d")).unwrap();
        let window = resolve_window(&cfg.checks[0].window, now);
        let meta = VerdictMeta {
            check: cfg.checks[0].name.clone(),
            description: None,
            actual_label: "Oxy net sales (wk)".to_string(),
            expected_label: "Toast net sales (wk)".to_string(),
            window: window.clone(),
        };
        let v = compare(&meta, 106_681.63, 149_056.91, &cfg.checks[0].tolerance, 5.0);
        assert_eq!(v.status, HealthStatus::Unhealthy);
        assert_eq!(v.window_start, "2026-07-12");
        assert_eq!(v.window_end, "2026-07-18");
        assert_eq!(v.window_timezone, "UTC");
    }

    #[tokio::test]
    async fn constant_vs_constant_compares_without_external_fetch() {
        let r = LiveReconcileRunner::from_env(chrono::Utc::now());
        let mut check = check_with_grain(Grain::Day);
        // Both sides constant: no measure runner, no external fetch needed.
        check.actual = constant_operand(0.0);
        check.expected = constant_operand(0.0);
        let window = resolve_window(&check.window, r.now);
        let v = r
            .verdict_for_check(
                uuid::Uuid::nil(),
                0,
                &check,
                &window,
                &None,
                &HashMap::new(),
            )
            .await;
        assert_eq!(v.status, HealthStatus::Healthy);
        assert_eq!(v.actual, 0.0);
        assert_eq!(v.expected, 0.0);
    }

    /// A warehouse stand-in: a scalar and a per-group result per measure, and
    /// the dimensions every grouped request asked for.
    struct FakeWarehouse {
        scalars: HashMap<String, f64>,
        groups: HashMap<String, Result<Vec<(Vec<String>, f64)>, String>>,
        grouped_dimensions: std::sync::Mutex<Vec<Vec<String>>>,
    }

    impl FakeWarehouse {
        fn new(
            scalars: &[(&str, f64)],
            groups: Vec<(&str, Result<Vec<(Vec<String>, f64)>, String>)>,
        ) -> Arc<Self> {
            Arc::new(Self {
                scalars: scalars.iter().map(|(m, v)| (m.to_string(), *v)).collect(),
                groups: groups
                    .into_iter()
                    .map(|(m, g)| (m.to_string(), g))
                    .collect(),
                grouped_dimensions: Default::default(),
            })
        }
    }

    use agentic_analytics::MetricTreeRunnerError;
    use oxy_airlayer_compat::engine::query::{QueryFilter, QueryRequest};

    #[async_trait::async_trait]
    impl MetricTreeRunner for FakeWarehouse {
        async fn load_layer(
            &self,
        ) -> Result<oxy_airlayer_compat::SemanticLayer, MetricTreeRunnerError> {
            Err(MetricTreeRunnerError::LayerLoad("unused".into()))
        }
        async fn list_databases(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
            vec![]
        }
        async fn run_explain(
            &self,
            _: String,
            _: String,
            _: (String, String),
            _: (String, String),
            _: Vec<QueryFilter>,
            _: oxy_airlayer_compat::engine::metric_tree_ops::ExplainConfig,
        ) -> Result<
            oxy_airlayer_compat::engine::metric_tree_ops::ExplainResult,
            MetricTreeRunnerError,
        > {
            Err(MetricTreeRunnerError::Op("unused".into()))
        }
        async fn run_opportunity(
            &self,
            _: String,
            _: String,
            _: (String, String),
            _: oxy_airlayer_compat::engine::metric_tree_ops::BenchmarkStatistic,
        ) -> Result<
            oxy_airlayer_compat::engine::metric_tree_ops::OpportunityResult,
            MetricTreeRunnerError,
        > {
            Err(MetricTreeRunnerError::Op("unused".into()))
        }
        async fn get_dimension_values(
            &self,
            _: String,
            _: String,
            _: u32,
        ) -> Result<Vec<String>, MetricTreeRunnerError> {
            Err(MetricTreeRunnerError::Op("unused".into()))
        }
        async fn run_time_series(
            &self,
            _: String,
            _: String,
            _: String,
            _: (String, String),
            _: Vec<QueryFilter>,
            _: Option<String>,
        ) -> Result<Vec<(String, f64)>, MetricTreeRunnerError> {
            Err(MetricTreeRunnerError::Op("unused".into()))
        }
        async fn run_query_scalar(
            &self,
            request: QueryRequest,
        ) -> Result<f64, MetricTreeRunnerError> {
            // A grouped request must never reach the scalar path.
            assert!(request.dimensions.is_empty());
            Ok(self.scalars[&request.measures[0]])
        }
        async fn run_query_grouped(
            &self,
            request: QueryRequest,
            keys: Vec<String>,
        ) -> Result<Vec<(Vec<String>, f64)>, MetricTreeRunnerError> {
            self.grouped_dimensions.lock().unwrap().push(keys);
            self.groups[&request.measures[0]]
                .clone()
                .map_err(MetricTreeRunnerError::Op)
        }
    }

    fn group_rows(rows: &[(&[&str], f64)]) -> Vec<(Vec<String>, f64)> {
        rows.iter()
            .map(|(k, v)| (k.iter().map(|s| s.to_string()).collect(), *v))
            .collect()
    }

    fn grouped_check(actual: Operand, expected: Operand, group_by: &[&str]) -> ReconcileCheck {
        let mut check = check_with_grain(Grain::Day);
        check.actual = actual;
        check.expected = expected;
        check.group_by = group_by.iter().map(|s| s.to_string()).collect();
        check
    }

    async fn run(check: &ReconcileCheck, warehouse: &Arc<FakeWarehouse>) -> DriftVerdict {
        let r = LiveReconcileRunner::from_env(chrono::Utc::now());
        let window = resolve_window(&check.window, r.now);
        r.verdict_for_check(
            uuid::Uuid::nil(),
            0,
            check,
            &window,
            &Some(warehouse.clone() as Arc<dyn MetricTreeRunner>),
            &HashMap::new(),
        )
        .await
    }

    #[tokio::test]
    async fn grouped_check_that_agrees_passes_with_no_disputes() {
        let wh = FakeWarehouse::new(
            &[("m.x", 150.0), ("m.y", 150.0)],
            vec![
                ("m.x", Ok(group_rows(&[(&["a"], 100.0), (&["b"], 50.0)]))),
                ("m.y", Ok(group_rows(&[(&["a"], 100.0), (&["b"], 50.0)]))),
            ],
        );
        let check = grouped_check(semantic_operand("m.x"), semantic_operand("m.y"), &["store"]);
        let v = run(&check, &wh).await;
        assert_eq!(v.status, HealthStatus::Healthy);
        let seg = v.segments.expect("a grouped check reports segments");
        assert_eq!(seg.group_by, vec!["store"]);
        assert_eq!(seg.groups_compared, 2);
        assert_eq!(seg.groups_disputed, 0);
        assert!(seg.disputed.is_empty());
    }

    #[tokio::test]
    async fn grouped_check_that_fails_names_its_groups_and_keeps_the_total() {
        // Groups sum to 140, but the verdict compares the ungrouped scalar
        // (130), exactly as without group_by.
        let wh = FakeWarehouse::new(
            &[("m.x", 130.0), ("m.y", 100.0)],
            vec![
                (
                    "m.x",
                    Ok(group_rows(&[
                        (&["a", "2026-07-01"], 60.0),
                        (&["b", "2026-07-01"], 80.0),
                    ])),
                ),
                (
                    "m.y",
                    Ok(group_rows(&[
                        (&["a", "2026-07-01"], 60.0),
                        (&["b", "2026-07-01"], 40.0),
                    ])),
                ),
            ],
        );
        let check = grouped_check(
            semantic_operand("m.x"),
            semantic_operand("m.y"),
            &["store", "other.date"],
        );
        let v = run(&check, &wh).await;
        let ungrouped = compare(
            &meta("c"),
            130.0,
            100.0,
            &check.tolerance,
            LiveReconcileRunner::from_env(chrono::Utc::now()).pct_unhealthy,
        );
        assert_eq!((v.actual, v.expected), (130.0, 100.0));
        assert_eq!(v.status, ungrouped.status);
        assert_eq!(v.reason, ungrouped.reason);
        let seg = v.segments.unwrap();
        assert_eq!(seg.group_by, vec!["store", "date"]);
        assert_eq!(seg.groups_disputed, 1);
        assert_eq!(seg.disputed[0].key, vec!["b", "2026-07-01"]);
        assert_eq!(seg.disputed[0].delta, 40.0);
        // Bare keys are qualified with the measure's view; dotted ones are not.
        let asked = wh_dimensions(&wh);
        assert_eq!(asked[0], vec!["m.store", "other.date"]);
    }

    fn wh_dimensions(wh: &FakeWarehouse) -> Vec<Vec<String>> {
        wh.grouped_dimensions.lock().unwrap().clone()
    }

    #[tokio::test]
    async fn a_group_on_one_side_is_disputed_against_zero() {
        let wh = FakeWarehouse::new(
            &[("m.x", 100.0)],
            vec![(
                "m.x",
                Ok(group_rows(&[(&["clean"], 0.0), (&["drifted"], 100.0)])),
            )],
        );
        let check = grouped_check(semantic_operand("m.x"), constant_operand(0.0), &["key"]);
        let v = run(&check, &wh).await;
        assert_eq!(v.status, HealthStatus::Unhealthy);
        let seg = v.segments.unwrap();
        assert_eq!(seg.groups_compared, 2);
        assert_eq!(seg.disputed.len(), 1);
        assert_eq!(seg.disputed[0].key, vec!["drifted"]);

        // Two semantic sides: the group only one returned reads 0 on the other.
        let wh = FakeWarehouse::new(
            &[("m.x", 100.0), ("m.y", 100.0)],
            vec![
                ("m.x", Ok(group_rows(&[(&["a"], 70.0), (&["late"], 30.0)]))),
                ("m.y", Ok(group_rows(&[(&["a"], 70.0)]))),
            ],
        );
        let check = grouped_check(semantic_operand("m.x"), semantic_operand("m.y"), &["store"]);
        let v = run(&check, &wh).await;
        assert_eq!(v.status, HealthStatus::Healthy, "the totals agree");
        let seg = v.segments.unwrap();
        assert_eq!(seg.disputed[0].key, vec!["late"]);
        assert_eq!(seg.disputed[0].expected, 0.0);
        assert_eq!(
            seg.disputed[0].missing,
            Some(oxy_workspace_health::reconcile::segments::MissingSide::Expected)
        );
    }

    #[tokio::test]
    async fn a_failed_grouped_query_keeps_the_verdict_and_says_why() {
        let wh = FakeWarehouse::new(
            &[("m.x", 100.0)],
            vec![("m.x", Err("no such dimension m.store".to_string()))],
        );
        let check = grouped_check(semantic_operand("m.x"), constant_operand(100.0), &["store"]);
        let v = run(&check, &wh).await;
        assert_eq!(v.status, HealthStatus::Healthy);
        let seg = v.segments.unwrap();
        assert!(seg.error.unwrap().contains("no such dimension m.store"));
        assert!(seg.disputed.is_empty());
    }

    #[tokio::test]
    async fn an_ungrouped_check_is_unchanged_on_the_wire() {
        let wh = FakeWarehouse::new(&[("m.x", 130.0)], vec![]);
        let check = check_with_grain(Grain::Day);
        let v = run(&check, &wh).await;
        assert!(v.segments.is_none());
        assert!(wh_dimensions(&wh).is_empty(), "no grouped query runs");
        let json = serde_json::to_value(&v).unwrap();
        let mut keys: Vec<&str> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec![
                "abs_diff",
                "actual",
                "actual_label",
                "check",
                "description",
                "expected",
                "expected_label",
                "pct_diff",
                "reason",
                "status",
                "window_end",
                "window_start",
                "window_timezone",
            ]
        );
        let expected = compare(
            &VerdictMeta {
                window: resolve_window(&check.window, chrono::Utc::now()),
                ..meta("c")
            },
            130.0,
            100.0,
            &check.tolerance,
            5.0,
        );
        assert_eq!(json, serde_json::to_value(&expected).unwrap());
    }

    #[tokio::test]
    async fn a_grouped_toast_operand_reads_its_groups_from_the_prefetched_report() {
        let wh = FakeWarehouse::new(
            &[("m.x", 350.0)],
            vec![(
                "m.x",
                Ok(group_rows(&[
                    (&["a", "2026-07-01"], 120.0),
                    (&["b", "2026-07-01"], 230.0),
                ])),
            )],
        );
        let mut external = constant_operand(0.0);
        external.constant = None;
        external.external = Some(ExternalSpec {
            source: "toast".to_string(),
            integration: None,
            metric: "net_sales".to_string(),
            restaurants: vec![],
            keys: [
                ("restaurant_id".to_string(), ExternalKey::Restaurant),
                ("business_date".to_string(), ExternalKey::BusinessDate),
            ]
            .into(),
        });
        let check = grouped_check(
            semantic_operand("m.x"),
            external,
            &["restaurant_id", "business_date"],
        );
        let externals = HashMap::from([(
            (0, Side::Expected),
            Ok(ExternalValue {
                total: 350.0,
                groups: Some(group_rows(&[
                    (&["a", "2026-07-01"], 150.0),
                    (&["b", "2026-07-01"], 200.0),
                ])),
            }),
        )]);
        let r = LiveReconcileRunner::from_env(chrono::Utc::now());
        let window = resolve_window(&check.window, r.now);
        let v = r
            .verdict_for_check(
                uuid::Uuid::nil(),
                0,
                &check,
                &window,
                &Some(wh.clone() as Arc<dyn MetricTreeRunner>),
                &externals,
            )
            .await;
        // The chain totals agree; two stores offset each other.
        assert_eq!(v.status, HealthStatus::Healthy);
        let seg = v.segments.unwrap();
        assert_eq!(seg.group_by, vec!["restaurant_id", "business_date"]);
        assert_eq!(seg.groups_disputed, 2);
        assert_eq!(seg.disputed[0].key, vec!["a", "2026-07-01"]);
        assert_eq!(seg.disputed[0].delta, -30.0);
        assert_eq!(seg.disputed[1].delta, 30.0);
        assert_eq!(
            wh_dimensions(&wh)[0],
            vec!["m.restaurant_id", "m.business_date"]
        );

        // The slot the batch fetch builds carries the mapped source keys.
        let slots = LiveReconcileRunner::collect_external_slots(
            std::slice::from_ref(&check),
            std::slice::from_ref(&window),
        );
        assert_eq!(
            slots[0].group_by,
            vec![ExternalKey::Restaurant, ExternalKey::BusinessDate]
        );
    }

    #[tokio::test]
    async fn a_bad_group_by_degrades_only_its_own_check() {
        let cfg = parse_reconcile_config(
            &serde_yaml::from_str(
                r#"
checks:
  - name: two_constants
    window: { last: 1, grain: day, offset: 1 }
    tolerance: { abs: 1.0, pct: 0.5 }
    group_by: [k]
    actual: { constant: 5 }
    expected: { constant: 5 }
  - name: stray_keys
    window: { last: 1, grain: day, offset: 1 }
    tolerance: { abs: 1.0, pct: 0.5 }
    actual: { constant: 100 }
    expected: { external: { source: toast, metric: net_sales, keys: { store: restaurant } } }
  - name: fine
    window: { last: 1, grain: day, offset: 1 }
    tolerance: { abs: 1.0, pct: 0.5 }
    actual: { constant: 5 }
    expected: { constant: 5 }
  - name: unkeyed_toast
    window: { last: 1, grain: day, offset: 1 }
    tolerance: { abs: 1.0, pct: 0.5 }
    group_by: sales.restaurant_id
    actual: { constant: 100 }
    expected: { external: { source: toast, metric: net_sales } }
"#,
            )
            .unwrap(),
        )
        .expect("a group_by problem must not fail the file");
        let externals = HashMap::from([
            (
                (1, Side::Expected),
                Ok(ExternalValue {
                    total: 100.0,
                    groups: None,
                }),
            ),
            (
                (3, Side::Expected),
                Ok(ExternalValue {
                    total: 100.0,
                    groups: None,
                }),
            ),
        ]);
        let r = LiveReconcileRunner::from_env(chrono::Utc::now());
        let mut verdicts = Vec::new();
        for (i, check) in cfg.checks.iter().enumerate() {
            let window = resolve_window(&check.window, r.now);
            verdicts.push(
                r.verdict_for_check(uuid::Uuid::nil(), i, check, &window, &None, &externals)
                    .await,
            );
        }

        let two_constants = &verdicts[0];
        assert_eq!(two_constants.status, HealthStatus::Degraded);
        let reason = two_constants.reason.as_deref().unwrap();
        assert!(reason.contains("both operands are constants"), "{reason}");
        assert!(two_constants.actual.is_nan(), "no comparison ran");

        let stray_keys = &verdicts[1];
        assert_eq!(stray_keys.status, HealthStatus::Degraded);
        let reason = stray_keys.reason.as_deref().unwrap();
        assert!(reason.contains("only applies with group_by"), "{reason}");

        assert_eq!(verdicts[2].status, HealthStatus::Healthy);
        assert!(verdicts[2].segments.is_none());

        // The pre-`keys` shape: compared on totals exactly as before, with the
        // reason group_by did nothing.
        let unkeyed = &verdicts[3];
        assert_eq!(unkeyed.status, HealthStatus::Healthy);
        assert_eq!((unkeyed.actual, unkeyed.expected), (100.0, 100.0));
        assert_eq!(unkeyed.reason, None);
        let seg = unkeyed.segments.as_ref().unwrap();
        assert!(seg.disputed.is_empty());
        assert!(seg.error.as_deref().unwrap().contains("has no `keys`"));
    }

    #[tokio::test]
    async fn a_key_naming_the_time_dimension_is_read_as_a_daily_bucket() {
        // `semantic_operand` binds the window to `m.d`; grouping by `d` must
        // read the day bucket, not the raw (possibly datetime) column.
        let wh = FakeWarehouse::new(
            &[("m.x", 100.0), ("m.y", 100.0)],
            vec![
                (
                    "m.x",
                    Ok(group_rows(&[(&["a", "2026-07-01T00:00:00.000"], 100.0)])),
                ),
                ("m.y", Ok(group_rows(&[(&["a", "2026-07-01"], 100.0)]))),
            ],
        );
        let check = grouped_check(
            semantic_operand("m.x"),
            semantic_operand("m.y"),
            &["store", "d"],
        );
        let v = run(&check, &wh).await;
        assert_eq!(wh_dimensions(&wh)[0], vec!["m.store", "m.d.day"]);
        let seg = v.segments.unwrap();
        assert_eq!(seg.groups_compared, 1, "the bucket joins a plain date");
        assert_eq!(seg.groups_disputed, 0);
    }
}
