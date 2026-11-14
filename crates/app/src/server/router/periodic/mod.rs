//! The periodic global driver's Cloud tick: each workspace is driven once per
//! interval by exactly one node, however many nodes run the driver.
//!
//! The driver loop itself lives in [`super::recovery::spawn_recovery`] and
//! runs on every node that drives the queue (every role but `serve`), every
//! `OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS` (30 s). It used to load every
//! workspace and build a project context for each on every tick on every
//! node, so a tick cost workspaces × nodes and all but one node's work per
//! workspace was wasted — the schedule and monitor ticks inside already
//! fired exactly once through their `next_run_at` compare-and-set, so the
//! losers paid for the context and found nothing due.
//!
//! Now a tick **claims** [`tick_batch`] workspaces at a time whose
//! `workspace_periodic_ticks.next_tick_at` has come due, with one
//! `FOR UPDATE SKIP LOCKED` statement that also moves each claimed row one
//! interval out ([`claim`]). Two nodes ticking at once lock disjoint sets, so
//! each workspace is driven by one node per interval and a tick's cost is
//! bounded by what it claims rather than by the workspace count. A full
//! batch means more may be due, so the tick claims again until a batch comes
//! back short or it has spent about one interval ([`TickDeps`] carries the
//! budget); one claim per sleep would cap the fleet at batch × nodes per
//! interval and fire schedules late past that. How far behind the fleet is
//! shows as `oxy_periodic_due_backlog`: the rows still due when the tick
//! yields, read once per tick after the last claim. Zero means the drain
//! caught up; a positive count means the budget ran out first. (Read before
//! the first claim it would count every workspace on a healthy fleet — a
//! claimed row comes due again exactly one interval later, which is when the
//! next tick starts.) A workspace with no row gets one lazily, due at a
//! random point in the first interval so the fleet's work stays spread.
//! There is still no leader: exactly-once is the row lock here and the
//! `next_run_at` CAS inside the schedule ticks, the two layered — the claim
//! decides who *looks*, the CAS decides who *fires*.
//!
//! What a tick does for each claimed workspace, in order: drive its stranded
//! runs (`recover_stranded_runs`, driver-lease-gated), fire its due cron
//! schedules, reconcile its schedule rows from the compiled revision when
//! that revision changed ([`reconcile`], which includes the `.monitor.yml`
//! bootstrap read from the compile boundary, [`monitors`]), then fire its due
//! monitor scans. After the batch, if it claimed anything, it runs the global
//! ticks that need no workspace context — health evals, pre-aggregation
//! cycles, experiments and the compile reconcile — each exactly-once by its
//! own CAS. [`metrics`] records how long it took and how much it claimed.
//!
//! Local mode has one workspace and no `workspaces` row, so it keeps its own
//! path in `recovery::recover_local`; it shares [`monitors::bootstrap_from_file`]
//! and [`reconcile::startup_housekeeping`] with this module.

pub mod claim;
pub(crate) mod metrics;
pub(crate) mod monitors;
pub(crate) mod reconcile;

use std::sync::Arc;
use std::time::Instant;

use agentic_pipeline::BuilderTestRunnerTrait;
use agentic_pipeline::platform::{BuilderBridges, PlatformContext};
use agentic_pipeline::recovery::{DrivePolicy, recover_stranded_runs};
use agentic_runtime::state::RuntimeState;
use sea_orm::DatabaseConnection;

use crate::agentic_wiring::build_builder_bridges;
use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
use crate::server::queued_analytics::SchemaCache;
use crate::server::queued_analytics::caller::CallerRunResolver;

use super::recovery::{self, CustomTasks};
use super::workspace_cache::WorkspaceContextCache;

/// How many workspaces one claim takes. A tick claims repeatedly while
/// batches come back full, so this sizes the unit of work between claims —
/// how many project contexts are built before another node gets a look at
/// what is due — not the tick's total. Raise the fleet, not this, when
/// `oxy_periodic_due_backlog` — the rows still due when a tick yielded —
/// stays above zero across ticks.
pub(crate) const TICK_BATCH_ENV: &str = "OXY_PERIODIC_TICK_BATCH";
const DEFAULT_TICK_BATCH: i64 = 50;

/// [`TICK_BATCH_ENV`], or the default when unset, empty or not a positive
/// integer.
pub(super) fn tick_batch() -> i64 {
    tick_batch_from(std::env::var(TICK_BATCH_ENV).ok().as_deref())
}

fn tick_batch_from(value: Option<&str>) -> i64 {
    value
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_TICK_BATCH)
}

/// What a tick needs to drive a workspace: the same handles
/// [`recovery::spawn_recovery`] threads into every driver loop, grouped so the
/// per-workspace step takes one argument instead of nine.
pub(super) struct TickDeps {
    pub db: DatabaseConnection,
    pub runtime: Arc<RuntimeState>,
    pub schema_cache: Option<SchemaCache>,
    pub builder_test_runner: Option<Arc<dyn BuilderTestRunnerTrait>>,
    pub router: Arc<dyn agentic_runtime::router::TaskRouter>,
    pub ws_cache: Arc<WorkspaceContextCache>,
    pub preagg: PreaggCacheCtx,
    pub custom_tasks: CustomTasks,
    pub policy: DrivePolicy,
}

/// One Cloud tick. Returns how many runs it drove or schedules it fired,
/// like the pass it replaced.
pub(super) async fn tick(deps: &TickDeps) -> usize {
    let started = Instant::now();
    let interval = recovery::inproc_global_worker_interval();
    let interval_secs = interval.as_secs() as i64;
    claim::seed_all(&deps.db, interval_secs).await;

    let (claimed, total) = drive_due(deps, tick_batch(), interval_secs, started, interval).await;
    // After the drain, not before it: a row claimed last tick comes due again
    // exactly as this tick starts, so a pre-claim count on a healthy fleet is
    // the workspace count. What is left due once the drain has yielded is
    // what the fleet could not drive inside the interval.
    match claim::due_backlog(&deps.db).await {
        Ok(due) => metrics::record_backlog(due),
        Err(e) => tracing::warn!(
            target: "recovery", error = %e, "periodic tick: reading the due backlog failed"
        ),
    }
    // The global ticks need no workspace context and are exactly-once by
    // their own CAS; a node that claimed nothing this tick leaves them to the
    // node that did, which keeps an idle node's tick at the claim alone.
    if claimed > 0 {
        tick_global(&deps.db).await;
    }
    metrics::record_tick(started.elapsed(), claimed);
    if claimed > 0 {
        tracing::debug!(
            target: "recovery",
            claimed,
            drove = total,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "periodic tick: drove the claimed workspaces"
        );
    }
    total
}

/// Claim and drive batches until one comes back short — nothing more is due
/// — or `budget` (one driver interval) is spent, so a tick that cannot keep up
/// yields to the next sleep instead of running without end. Returns how many
/// workspaces were claimed and how many runs or schedules they produced.
async fn drive_due(
    deps: &TickDeps,
    batch: i64,
    interval_secs: i64,
    started: Instant,
    budget: std::time::Duration,
) -> (usize, usize) {
    let (mut claimed_total, mut total) = (0usize, 0usize);
    loop {
        let claimed = claim::claim_batch(&deps.db, batch, interval_secs).await;
        for ws in &claimed {
            total += drive_workspace(deps, ws).await;
        }
        claimed_total += claimed.len();
        if (claimed.len() as i64) < batch || started.elapsed() >= budget {
            break;
        }
    }
    (claimed_total, total)
}

/// Drive one claimed workspace: stranded runs, cron schedules, the gated
/// schedule reconcile, then monitor scans.
async fn drive_workspace(deps: &TickDeps, ws: &claim::Claimed) -> usize {
    let db = &deps.db;
    let workspace_id = ws.workspace_id;
    let Some(path) = ws.path.as_deref() else {
        tracing::debug!(target: "recovery", %workspace_id, "periodic tick: workspace has no path yet");
        return 0;
    };
    // Tagged with the claim's promoted revision, same as the latency tick: the
    // two share this cache, so an untagged lookup here would serve (and an
    // untagged insert would leave) a context from before a promote.
    let Some(project_ctx) = super::recovery::worker_ctx::worker_project_ctx(
        &deps.ws_cache,
        workspace_id,
        path,
        ws.current_revision_id,
        db,
        &deps.preagg,
    )
    .await
    else {
        // build_cloud_project_ctx_inner already logged the failure cause.
        return 0;
    };
    let platform: Arc<dyn PlatformContext> = project_ctx.clone();
    let platform_for_monitor: Arc<dyn PlatformContext> = project_ctx.clone();
    // The scheduler tick needs a workspace handle: airway targets resolve
    // the pipeline file on THIS workspace's context.
    let workspace: Arc<dyn agentic_pipeline::WorkflowWorkspaceContext> = project_ctx.clone();
    let bridges: Option<BuilderBridges> = Some(build_builder_bridges(project_ctx));

    // Filter to THIS workspace's runs only — otherwise the loop would drive
    // every workspace's stranded rows with the wrong PlatformContext.
    let recovered = recover_stranded_runs(
        db.clone(),
        deps.runtime.clone(),
        platform,
        CallerRunResolver::shared(db),
        bridges,
        deps.schema_cache.clone(),
        deps.builder_test_runner.clone(),
        deps.router.clone(),
        Some(workspace_id),
        Some(recovery::build_custom_task_registry(
            db,
            &deps.preagg,
            &deps.custom_tasks,
            deps.schema_cache.clone(),
        )),
        deps.policy,
    )
    .await;
    let fired =
        agentic_pipeline::scheduler::tick_schedules(db, workspace_id, workspace.as_ref()).await;
    reconcile::reconcile_workspace(db, ws).await;
    let monitor_fired =
        agentic_pipeline::scheduler::tick_monitor_schedules(db, workspace_id, platform_for_monitor)
            .await;
    report_workspace(workspace_id, recovered, fired, monitor_fired);
    recovered + fired
}

fn report_workspace(workspace_id: uuid::Uuid, recovered: usize, fired: usize, monitors: usize) {
    if recovered > 0 {
        tracing::info!(
            target: "recovery",
            %workspace_id,
            recovered,
            "cloud recovery: resumed runs for workspace"
        );
    }
    if fired > 0 {
        tracing::info!(target: "scheduler", %workspace_id, fired, "cloud tick fired schedules");
    }
    if monitors > 0 {
        tracing::info!(
            target: "metric_monitoring",
            %workspace_id,
            fired = monitors,
            "monitor tick spawned scans"
        );
    }
}

/// The ticks that need no workspace context, once per pass: due health
/// evals, pre-aggregation cycles and experiment checks (each a cheap indexed
/// SELECT of due rows plus a CAS per fire), and the compile reconcile (which
/// does nothing unless `OXY_COMPILE_RECONCILE` is set; each check is claimed
/// in Postgres, so it does not matter how many nodes run it).
async fn tick_global(db: &DatabaseConnection) {
    let health_fired = agentic_pipeline::scheduler::tick_health_schedules(db).await;
    if health_fired > 0 {
        tracing::info!(target: "health_eval", "cloud health schedule fired");
    }
    let preagg_fired = agentic_pipeline::scheduler::tick_preagg_schedules(db).await;
    if preagg_fired > 0 {
        tracing::info!(target: "preagg", "cloud preagg schedule fired");
    }
    // The surfaces' global ticks (`SurfaceSeams::global_ticks`; the experiment
    // tick). Exactly-once per item across replicas by each tick's own CAS.
    super::seams::run_global_ticks(db, chrono::Utc::now()).await;
    crate::server::compile_reconcile::tick(db).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_batch_defaults_unless_set_to_a_positive_integer() {
        for unset in [None, Some(""), Some("0"), Some("-5"), Some("many")] {
            assert_eq!(tick_batch_from(unset), DEFAULT_TICK_BATCH, "{unset:?}");
        }
        assert_eq!(tick_batch_from(Some("10")), 10);
        assert_eq!(tick_batch_from(Some(" 200 ")), 200);
    }
}
