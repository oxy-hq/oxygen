//! Startup recovery: resume in-flight tasks after server crash/restart.
//!
//! Uses a top-down tree-walk approach:
//! 1. Reconstruct coordinator from DB via `from_db()`
//! 2. Decide what continues the root (`root_entry`) and each parent whose
//!    children have all reported (`children_done`) — one continuation each
//! 3. Walk the task tree, classify each task
//! 4. Re-launch tasks that have checkpoints and that nothing else continues
//! 5. Mark stale tasks as failed (parent will re-delegate)
//! 6. Process PendingResumes (children done, parent not yet resumed)

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agentic_core::hub_task::spawn_with_hub;
use agentic_runtime::coordinator::Coordinator;
use agentic_runtime::state::RuntimeState;
use agentic_runtime::transport::DurableTransport;
use agentic_runtime::worker::Worker;
use sea_orm::DatabaseConnection;

use crate::drive_policy::report_fallback_takes;
use crate::executor::PipelineTaskExecutor;
use crate::platform::preview::platform_for_root;
use crate::platform::{BuilderBridges, PlatformContext, RunPlatformResolver};

mod children_done;
mod root_entry;

use root_entry::RootEntry;

/// How many times the recovery *loops* may re-drive one run before retiring it.
///
/// Four, not "until it works": every re-drive that fails the same way is a
/// re-drive that will keep failing, and an unbounded retry of a run nobody is
/// waiting on is indistinguishable — to Postgres — from a denial of service by
/// your own control plane.
///
/// **This is a lifetime total, and only loop-driven recoveries spend it** (see
/// [`RecoveryOrigin`]). A user retry through `reset_run_for_retry` zeroes the
/// counter, which is the right place for "try harder": a human decided.
const MAX_RECOVERY_ATTEMPTS: i32 = 4;

/// Which path is recovering this run — and therefore whether it spends budget.
///
/// The distinction exists because a lifetime cap over *all* recoveries would
/// kill healthy work: a long-lived automation cleanly resumed across four
/// rolling deploys would be dead-lettered on the fifth, having never failed at
/// anything. Those recoveries arrive through [`RecoveryOrigin::Startup`], one
/// per process start, and are free.
///
/// The runaway this bounds is the other shape entirely — the periodic and
/// latency loops re-selecting the same unrunnable run several times a second,
/// forever. Only they spend budget, and four of their attempts elapse in about
/// a second, so a genuinely stuck run retires almost immediately while a
/// restarted one is untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryOrigin {
    /// Process startup: one recovery per run per boot. Not budgeted.
    Startup,
    /// The periodic / latency driver loops, which re-select on every tick.
    Loop,
}

/// What a recovery pass actually did — so a dead-letter is not counted, or
/// logged, as a successful drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecoveryOutcome {
    /// The run was handed to a coordinator.
    Drove,
    /// The run exhausted [`MAX_RECOVERY_ATTEMPTS`] and was dead-lettered.
    Retired,
}

/// Has this run spent its recovery budget?
///
/// Split out so the boundary is unit-testable without a database — off-by-one
/// here is the difference between "retried four times" and "retried forever".
fn recovery_budget_exhausted(attempt: i32) -> bool {
    attempt > MAX_RECOVERY_ATTEMPTS
}

/// Did recovery fail because the run can never be resumed?
///
/// A run interrupted before its first checkpoint (analytics, builder, app
/// function) or with no saved state (automation) cannot be resumed; the
/// executor says so and the run is retired. Every deploy that lands mid-run
/// produces some, so this is the expected path, not recovery breaking — and at
/// ERROR it was the third-largest issue in Sentry on 2026-09-15
/// (`failed to recover run`, 83 events in five hours on dev). Anything else
/// stays ERROR.
fn is_unresumable(err: &str) -> bool {
    err.contains(crate::executor::NO_CHECKPOINT) || err.contains(crate::executor::NO_SAVED_STATE)
}

/// Log a failed recovery at the level it deserves — see [`is_unresumable`].
/// The run is retired by the caller either way.
fn log_recovery_failure(run_id: &str, err: &str, message: &'static str) {
    if is_unresumable(err) {
        tracing::warn!(
            target: "recovery",
            run_id,
            error = %err,
            "{message}: the run cannot be resumed, retiring it"
        );
    } else {
        tracing::error!(target: "recovery", run_id, error = %err, "{message}");
    }
}

/// Recover all in-flight runs on server startup.
///
/// `workspace_id` — when `Some`, only resume runs owned by that
/// workspace (cloud-mode startup iterates per workspace, using the
/// per-workspace `PlatformContext` so a recovered run never gets driven
/// with the wrong workspace's connectors/secrets). `None` resumes every
/// workspace's runs — appropriate for local mode (single workspace) and
/// tests.
///
/// `run_platform` picks the platform each root is driven with (`platform` is
/// the base it may hand back) — see [`crate::platform::preview`]. A root it
/// cannot answer for is skipped, never driven with the base.
///
/// `policy` — see [`leave_queued_work_for_the_fleet`]: applied only to the
/// roots another node's latency worker can see, never to the rest.
#[allow(clippy::too_many_arguments)]
pub async fn recover_active_runs(
    db: DatabaseConnection,
    state: Arc<RuntimeState>,
    platform: Arc<dyn PlatformContext>,
    run_platform: Arc<dyn RunPlatformResolver>,
    builder_bridges: Option<BuilderBridges>,
    schema_cache: Option<Arc<Mutex<HashMap<String, agentic_analytics::SchemaCatalog>>>>,
    builder_test_runner: Option<Arc<dyn agentic_builder::BuilderTestRunner>>,
    builder_app_runner: Option<Arc<dyn agentic_builder::BuilderAppRunner>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    workspace_id: Option<uuid::Uuid>,
    custom_executors: Option<Arc<agentic_runtime::worker::CustomTaskRegistry>>,
    policy: DrivePolicy,
) -> usize {
    // Pre-pass: clean up stale queue entries from the previous server lifetime.
    // Tasks "claimed" by now-dead workers get re-queued or dead-lettered.
    let transport = DurableTransport::with_router(db.clone(), router.clone(), None);
    let reaped = transport.run_reaper().await.total();
    if reaped > 0 {
        tracing::info!(target: "recovery", count = reaped, "reaper pre-pass: cleaned stale queue entries");
    }

    let roots = match agentic_runtime::crud::get_resumable_root_runs(&db, workspace_id).await {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(target: "recovery", error = %e, "failed to query resumable runs");
            return 0;
        }
    };
    let roots = leave_queued_work_for_the_fleet(&db, roots, workspace_id, policy).await;

    if roots.is_empty() {
        return 0;
    }

    tracing::info!(target: "recovery", count = roots.len(), "found resumable runs");

    let mut recovered = 0;
    for root in roots {
        let run_id = root.id.clone();
        let Some(root_platform) = platform_for_root(&*run_platform, &root, &platform).await else {
            continue;
        };
        match recover_single_run(
            &root,
            db.clone(),
            state.clone(),
            root_platform,
            builder_bridges.clone(),
            schema_cache.clone(),
            builder_test_runner.clone(),
            builder_app_runner.clone(),
            router.clone(),
            custom_executors.clone(),
            RecoveryOrigin::Startup,
        )
        .await
        {
            Ok(RecoveryOutcome::Drove) => {
                recovered += 1;
                tracing::info!(target: "recovery", run_id = %run_id, "run recovered");
            }
            // Deliberately not counted: retiring a run is the guard firing, not
            // work getting done. Counting it would hide the one signal that
            // tells an operator this bound is active.
            Ok(RecoveryOutcome::Retired) => {}
            Err(e) => {
                log_recovery_failure(&run_id, &e, "failed to recover run");
                // `retire_run`, not `mark_recovery_failed`: failing the run
                // alone leaves its queue rows `queued`, and `claim_task` has no
                // run-status predicate — so a worker can still claim and execute
                // a task belonging to a run that is already dead.
                agentic_runtime::crud::retire_run(&db, &run_id, &e)
                    .await
                    .ok();
            }
        }
    }

    recovered
}

/// The startup pass's share of the placement gate: drop from `roots` every run
/// this process defers **and** another node's latency worker can see.
///
/// `get_resumable_root_runs` is not scoped to this process's own work: it
/// selects every unleased active root in the workspace, and the reaper
/// pre-pass has just re-queued every dead worker's claim. So a deferring `ide`
/// that boots while an OOM-killed worker's pipeline is in that state would
/// drive it here, ungated — the heavy run the flag exists to keep off this
/// node, by the one selection the latency worker and the stranded tick do not
/// share.
///
/// Only the roots [`find_pending_global_runs`] also selects are candidates,
/// and that is what makes declining one safe: a run with a `queued` Global row
/// and no live lease is exactly what every node's latency worker polls for, a
/// worker takes it within a tick, and this node's own latency worker takes it
/// after [`STRANDED_GRACE_SECS`] if none does. A root with no such row — an
/// analytics or builder run this process was direct-driving when it died —
/// has no other loop guaranteed to see it, so it stays here whatever its kind.
///
/// Fails open: if the pending set cannot be read, every root is kept, which is
/// this pass's behaviour without a policy.
///
/// [`find_pending_global_runs`]: agentic_runtime::crud::find_pending_global_runs
async fn leave_queued_work_for_the_fleet(
    db: &DatabaseConnection,
    mut roots: Vec<agentic_runtime::entity::run::Model>,
    workspace_id: Option<uuid::Uuid>,
    policy: DrivePolicy,
) -> Vec<agentic_runtime::entity::run::Model> {
    if roots.is_empty() || policy == DrivePolicy::ALL {
        return roots;
    }
    let pending = match agentic_runtime::crud::find_pending_global_runs(db, workspace_id).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                error = %e,
                "startup pass: could not read the pending Global set; resuming every root"
            );
            return roots;
        }
    };
    let (drivable, declined) = partition_drivable(pending, policy);
    // Only the roots this pass resumes: a pending run that is not a resumable
    // root (`waiting_on_child`) is not driven here, and the latency worker
    // reports it when it takes it.
    {
        let resumed: std::collections::HashSet<&str> =
            roots.iter().map(|r| r.id.as_str()).collect();
        let resumed: Vec<_> = drivable
            .into_iter()
            .filter(|r| resumed.contains(r.run_id.as_str()))
            .collect();
        report_fallback_takes(&resumed, policy, "startup pass");
    }
    if declined.is_empty() {
        return roots;
    }
    let declined: std::collections::HashSet<String> =
        declined.into_iter().map(|r| r.run_id).collect();
    let before = roots.len();
    roots.retain(|root| !declined.contains(&root.id));
    if roots.len() < before {
        tracing::info!(
            target: "recovery",
            count = before - roots.len(),
            "startup pass: leaving queued runs this process defers for the worker fleet"
        );
    }
    roots
}

/// Periodic global-driver entrypoint: drive **stranded** runs only.
///
/// Unlike [`recover_active_runs`] (startup: every coordinator is dead, so
/// it selects via `get_resumable_root_runs`), this is called repeatedly
/// while the process is alive. It must never touch a run a live per-request
/// coordinator is driving, so it selects via `find_stuck_runs` — which
/// requires `NOT EXISTS (active queue entry)`. A live interactive run
/// always has a `claimed`/heart-beating queue entry and is excluded by
/// construction. The reaper pre-pass first frees entries claimed by dead
/// workers, so a crashed interactive run becomes stranded → eligible.
///
/// `workspace_id` — see [`recover_active_runs`]. The cloud-mode periodic
/// loop passes the iteration's workspace id so it never drives a
/// foreign workspace's row with this context.
///
/// `run_platform` — see [`recover_active_runs`].
///
/// `policy` — which run kinds this process drives (see [`DrivePolicy`]), the
/// same value the latency worker applies. It has to be applied **here** too,
/// because this pass is the one that reaps: the pre-pass below flips a dead
/// worker's `claimed` row back to `queued`, and `find_stuck_runs` then selects
/// that very run in the same call — a long pipeline's `updated_at` is already
/// past the grace, and the worker that died was its lease-holder, so once that
/// lease has lapsed nothing else excludes it. Ungated, an OOM-killed airway run
/// landed on whichever node's tick reaped it, which under
/// [`DrivePolicy::Only`] is exactly the node that was configured not to run
/// it. The gate reads [`StuckRun::unclaimed_secs`](agentic_runtime::crud::StuckRun),
/// whose clock restarts at the reaper's re-queue, so a deferring node leaves
/// the run for the fleet for one grace and takes it only if nobody did.
/// `Except` is a no-op on this path by construction (the selection is
/// `workflow` + `airway`, never `compile`), so a worker's tick is unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn recover_stranded_runs(
    db: DatabaseConnection,
    state: Arc<RuntimeState>,
    platform: Arc<dyn PlatformContext>,
    run_platform: Arc<dyn RunPlatformResolver>,
    builder_bridges: Option<BuilderBridges>,
    schema_cache: Option<Arc<Mutex<HashMap<String, agentic_analytics::SchemaCatalog>>>>,
    builder_test_runner: Option<Arc<dyn agentic_builder::BuilderTestRunner>>,
    builder_app_runner: Option<Arc<dyn agentic_builder::BuilderAppRunner>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    workspace_id: Option<uuid::Uuid>,
    custom_executors: Option<Arc<agentic_runtime::worker::CustomTaskRegistry>>,
    policy: DrivePolicy,
) -> usize {
    // Free entries claimed by workers that died — turns a crashed
    // interactive run into a stranded one this same pass.
    let transport = DurableTransport::with_router(db.clone(), router.clone(), None);
    let reaped = transport.run_reaper().await.total();
    if reaped > 0 {
        tracing::info!(target: "recovery", count = reaped, "global loop: reaped stale queue entries");
    }

    let stuck = match agentic_runtime::crud::find_stuck_runs(&db, STRANDED_GRACE_SECS, workspace_id)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            tracing::error!(target: "recovery", error = %e, "global loop: find_stuck_runs failed");
            return 0;
        }
    };
    // Before the lease, like the latency worker: a declined run keeps
    // `driver_id IS NULL` and the next node's tick selects it.
    let (stuck, declined) = partition_drivable(stuck, policy);
    if !declined.is_empty() {
        tracing::debug!(
            target: "recovery",
            count = declined.len(),
            "global loop: leaving stranded runs this process defers for a node that takes them"
        );
    }
    report_fallback_takes(&stuck, policy, "global loop");
    if stuck.is_empty() {
        return 0;
    }
    tracing::info!(target: "recovery", count = stuck.len(), "global loop: found stranded runs");

    let mut recovered = 0;
    for s in stuck {
        let root = match agentic_runtime::crud::get_run(&db, &s.run_id).await {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(target: "recovery", run_id = %s.run_id, error = %e, "global loop: get_run failed");
                continue;
            }
        };
        let Some(root_platform) = platform_for_root(&*run_platform, &root, &platform).await else {
            continue;
        };
        match recover_single_run(
            &root,
            db.clone(),
            state.clone(),
            root_platform,
            builder_bridges.clone(),
            schema_cache.clone(),
            builder_test_runner.clone(),
            builder_app_runner.clone(),
            router.clone(),
            custom_executors.clone(),
            RecoveryOrigin::Loop,
        )
        .await
        {
            Ok(RecoveryOutcome::Drove) => {
                recovered += 1;
                tracing::info!(target: "recovery", run_id = %s.run_id, "global loop: drove stranded run");
            }
            Ok(RecoveryOutcome::Retired) => {}
            Err(e) => {
                log_recovery_failure(&s.run_id, &e, "global loop: failed to drive stranded run");
                // See `recover_active_runs` — retire, don't merely fail, or the
                // run's queue rows stay claimable after the run is dead.
                agentic_runtime::crud::retire_run(&db, &s.run_id, &e)
                    .await
                    .ok();
            }
        }
    }
    recovered
}

// The placement gate lives in `drive_policy`; re-exported so the path the
// tests, the host and the docs already name (`recovery::may_drive`) is the
// one definition.
pub use crate::drive_policy::{DrivePolicy, STRANDED_GRACE_SECS, may_drive, partition_drivable};

/// §12 FU4c latency-worker entrypoint. Drives runs that already have a
/// `queued scope_owned = false` queue entry — freshly-seeded Global runs
/// (scheduler tick / `run-now`) — at claim-time, without the periodic
/// loop's grace window. The driving mechanism is the same as
/// `recover_stranded_runs` (lease-CAS-acquire + spawn coordinator); the
/// only difference is the selection predicate.
///
/// Safe to interleave with the periodic loop: both call
/// `recover_single_run`, which CAS-acquires the driver lease — the loser
/// of any race skips cleanly. No double-drive.
///
/// `workspace_id` — see [`recover_active_runs`]. The local-mode latency
/// worker passes `None` (single workspace); the cloud-mode worker passes
/// the iteration's workspace id so it routes per-row to the right
/// `PlatformContext`.
///
/// `policy` — which run kinds this process drives, matched on `source_type`
/// (see [`DrivePolicy`]). Checked HERE rather than by the caller because this is where
/// the driver lease is taken. A caller-side filter covers only the selection
/// the caller made; this function re-selects per workspace, and it is that
/// second selection which feeds `recover_single_run` → `try_acquire_driver`.
/// Filtering upstream therefore misses any workspace holding declined work
/// alongside work this process CAN drive — the common shape, since health,
/// preagg and schedule ticks seed Global runs per workspace continuously.
///
/// Declining before the lease is what makes the handoff work: the run keeps
/// `driver_id IS NULL`, so a node that can drive it selects it on its next
/// tick. Declining AFTER the claim cannot hand off at all — only the
/// lease-holder may claim the row, so it re-selects its own work while its
/// heartbeat excludes everyone else. See [`may_drive`].
///
/// `run_platform` — see [`recover_active_runs`]. Asked after the policy and
/// before the lease, so a root it cannot answer for keeps `driver_id IS NULL`.
#[allow(clippy::too_many_arguments)]
pub async fn recover_pending_global_runs(
    db: DatabaseConnection,
    state: Arc<RuntimeState>,
    platform: Arc<dyn PlatformContext>,
    run_platform: Arc<dyn RunPlatformResolver>,
    builder_bridges: Option<BuilderBridges>,
    schema_cache: Option<Arc<Mutex<HashMap<String, agentic_analytics::SchemaCatalog>>>>,
    builder_test_runner: Option<Arc<dyn agentic_builder::BuilderTestRunner>>,
    builder_app_runner: Option<Arc<dyn agentic_builder::BuilderAppRunner>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    workspace_id: Option<uuid::Uuid>,
    custom_executors: Option<Arc<agentic_runtime::worker::CustomTaskRegistry>>,
    policy: DrivePolicy,
) -> usize {
    let pending = match agentic_runtime::crud::find_pending_global_runs(&db, workspace_id).await {
        Ok(p) => p,
        Err(e) => {
            // A DB failure while shutting down is expected (the co-located DB
            // goes down with the process on Ctrl-C in local/dev), not an error
            // — keep it loud only in steady state.
            if agentic_runtime::transport::is_shutting_down() {
                tracing::debug!(target: "recovery", error = %e, "latency loop: find_pending_global_runs failed during shutdown");
            } else {
                tracing::error!(target: "recovery", error = %e, "latency loop: find_pending_global_runs failed");
            }
            return 0;
        }
    };
    let (pending, declined) = partition_drivable(pending, policy);
    if !declined.is_empty() {
        tracing::debug!(
            target: "recovery",
            count = declined.len(),
            "latency loop: leaving runs this process cannot drive for a node that can"
        );
    }
    report_fallback_takes(&pending, policy, "latency loop");
    if pending.is_empty() {
        return 0;
    }
    tracing::debug!(target: "recovery", count = pending.len(), "latency loop: found pending Global runs");

    let mut driven = 0;
    for s in pending {
        let root = match agentic_runtime::crud::get_run(&db, &s.run_id).await {
            Ok(Some(r)) => r,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(target: "recovery", run_id = %s.run_id, error = %e, "latency loop: get_run failed");
                continue;
            }
        };
        let Some(root_platform) = platform_for_root(&*run_platform, &root, &platform).await else {
            continue;
        };
        match recover_single_run(
            &root,
            db.clone(),
            state.clone(),
            root_platform,
            builder_bridges.clone(),
            schema_cache.clone(),
            builder_test_runner.clone(),
            builder_app_runner.clone(),
            router.clone(),
            custom_executors.clone(),
            RecoveryOrigin::Loop,
        )
        .await
        {
            Ok(RecoveryOutcome::Drove) => {
                driven += 1;
                tracing::debug!(target: "recovery", run_id = %s.run_id, "latency loop: drove Global run");
            }
            Ok(RecoveryOutcome::Retired) => {}
            Err(e) => {
                log_recovery_failure(&s.run_id, &e, "latency loop: failed to drive Global run");
                // See `recover_active_runs` — retire, don't merely fail, or the
                // run's queue rows stay claimable after the run is dead.
                agentic_runtime::crud::retire_run(&db, &s.run_id, &e)
                    .await
                    .ok();
            }
        }
    }
    driven
}

#[allow(clippy::too_many_arguments)]
async fn recover_single_run(
    root: &agentic_runtime::entity::run::Model,
    db: DatabaseConnection,
    state: Arc<RuntimeState>,
    platform: Arc<dyn PlatformContext>,
    builder_bridges: Option<BuilderBridges>,
    schema_cache: Option<Arc<Mutex<HashMap<String, agentic_analytics::SchemaCatalog>>>>,
    builder_test_runner: Option<Arc<dyn agentic_builder::BuilderTestRunner>>,
    builder_app_runner: Option<Arc<dyn agentic_builder::BuilderAppRunner>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    custom_executors: Option<Arc<agentic_runtime::worker::CustomTaskRegistry>>,
    origin: RecoveryOrigin,
) -> Result<RecoveryOutcome, String> {
    // Acquire the driver lease before touching the run. If a *live* driver
    // already owns it (another replica, or — once Task 6 lands — a
    // concurrent recovery tick), skip: driving it here would double-drive
    // the run. The lease auto-clears on terminal `transition_run`; if this
    // recovery is itself interrupted the lease goes stale and the run
    // becomes resumable again. Heartbeating the lease while driving is the
    // periodic loop's job (Task 6).
    let driver_id = format!("recovery-{}", uuid::Uuid::new_v4());
    match agentic_runtime::crud::try_acquire_driver(&db, &root.id, &driver_id).await {
        Ok(true) => {
            // We now own this run: clear any stale "server restarted: run
            // will be resumed automatically" note `cleanup_stale_runs` left
            // behind (or any other prior error) so the UI doesn't keep
            // showing a red "Pipeline error" banner over a run that's
            // actually being resumed. Best-effort — a failure here shouldn't
            // block the resume itself, just leaves the stale note in place.
            if let Err(e) = agentic_runtime::crud::clear_run_error(&db, &root.id).await {
                tracing::warn!(
                    target: "recovery",
                    run_id = %root.id,
                    error = %e,
                    "failed to clear stale error_message on recovery claim"
                );
            }
        }
        Ok(false) => {
            tracing::info!(
                target: "recovery",
                run_id = %root.id,
                "skipping recovery: a live driver already holds the lease"
            );
            // Not a drive; the holder is doing the work. Reported as `Drove` so
            // the counters keep their existing meaning ("this run is in hand"),
            // which is what they meant before this enum existed.
            return Ok(RecoveryOutcome::Drove);
        }
        Err(e) => return Err(format!("driver lease acquire failed: {e}")),
    }

    // A run started in a workspace preview is never recovered: its platform
    // (the branch, every write held) died with the process that drove it, and
    // the platform here would be production's. Retired as the lease owner, on
    // every entry point, before any budget or state is touched.
    if crate::platform::preview_stamp::is_stamped(root.metadata.as_ref()) {
        crate::platform::preview_stamp::retire_interrupted(&db, &root.id).await?;
        return Ok(RecoveryOutcome::Retired);
    }

    // Spend one unit of the run's recovery budget, and retire it if that was the
    // last one. This is the *only* bound on how many times a loop can re-drive a
    // Global run: `find_pending_global_runs` re-selects a non-terminal run on
    // every tick, so absent a budget a run that cannot make progress is
    // re-driven forever — which is how prod (2026-09-01) ended up with one
    // `SELECT` accounting for 87% of all database load.
    //
    // The driver lease acquired above is what keeps the count meaningful: a run
    // being actively driven holds the lease and is not re-selected, and one
    // parked on human input spends nothing either. Only a genuine *re*-recovery
    // costs a unit. That is what `agentic_runs.attempt` was added to count
    // ("incremented on each recovery"), and the coordinator already reads it
    // back expecting the recovery caller to have done this — nothing in
    // production ever did.
    if origin == RecoveryOrigin::Loop {
        match agentic_runtime::crud::increment_attempt(&db, &root.id).await {
            Ok(attempt) if recovery_budget_exhausted(attempt) => {
                retire_exhausted_run(&db, &root.id, attempt).await;
                // Same release the failure path below performs. `retire_run`
                // makes the run terminal, so the acquire CAS would reclaim the
                // lease anyway — but leaving it to that means the two exits from
                // a dead airway run behave differently for no reason, and the
                // difference is invisible until someone is holding a stale lease
                // wondering which path produced it.
                if root.source_type.as_deref() == Some("airway") {
                    tracing::info!(
                        target: "recovery",
                        run_id = %root.id,
                        "airway run retired on budget exhaustion; releasing its \
                         single-flight lease"
                    );
                    crate::airway_run::release_airway_lease(&db, &root.id).await;
                }
                return Ok(RecoveryOutcome::Retired);
            }
            Ok(attempt) => tracing::debug!(
                target: "recovery",
                run_id = %root.id,
                attempt,
                max = MAX_RECOVERY_ATTEMPTS,
                "recovery attempt"
            ),
            // Can't count — behave exactly as before rather than killing a live
            // run over a transient DB error. Loud, because an unbudgeted run is
            // the failure mode this guard exists to prevent.
            Err(e) => tracing::warn!(
                target: "recovery",
                run_id = %root.id,
                error = %e,
                "could not record a recovery attempt; this run is proceeding UNBUDGETED"
            ),
        }
    }

    // The driver lease is OWNED from here, so this frame is the run's owner and
    // may release run-scoped resources on failure. Split out so that ownership
    // is expressed by the call boundary rather than by remembering which of the
    // six error exits below the acquire happened to be.
    let outcome = recover_single_run_owned(
        root,
        db.clone(),
        state,
        platform,
        builder_bridges,
        schema_cache,
        builder_test_runner,
        builder_app_runner,
        router,
        custom_executors,
        driver_id,
    )
    .await;

    // An airway run that fails recovery is force-failed by the caller
    // (`mark_recovery_failed`), but its single-flight lease was taken at SUBMIT
    // and nothing else frees it — so the pipeline stayed blocked for the full
    // 6h TTL. `resume_from_state` dispatches airway into its `_` arm, which
    // resumes only from `suspend_data`, and airway runs never suspend — so
    // recovery CANNOT resume one and always force-fails it.
    //
    // SCOPE — narrower than it first appears, and measured rather than
    // inferred. Reaching `resume_from_state` at all requires the run's queued
    // task row to be GONE (claimed by a worker that died, or dead-lettered).
    // `is_root_with_queued_entry` below short-circuits the common case and
    // leaves the root for the worker, so a merely interrupted run is re-driven
    // and leaks nothing — a diagnostic run confirmed it stays `running` with no
    // error and never reaches the caller's `mark_recovery_failed`. An earlier
    // revision of this comment claimed "every restart, six hours, every time";
    // that was wrong, and it matters here because the queueing redesign will
    // weigh this site when deciding whether claim-time acquisition makes it
    // redundant.
    //
    // Safe without a liveness predicate precisely because we hold the driver
    // lease: this frame already concluded the run is dead, and recovery would
    // have re-driven it on the success path — a strictly stronger act than
    // releasing a lease. The acquire-error exit above returns before this point
    // and never releases, because there we own nothing.
    if outcome.is_err() && root.source_type.as_deref() == Some("airway") {
        tracing::info!(
            target: "recovery",
            run_id = %root.id,
            "airway recovery failed; releasing its single-flight lease"
        );
        crate::airway_run::release_airway_lease(&db, &root.id).await;
    }

    outcome.map(|()| RecoveryOutcome::Drove)
}

/// Dead-letter a run that has exhausted [`MAX_RECOVERY_ATTEMPTS`].
///
/// The two writes — cancelling the queue rows and marking the run terminal —
/// go in ONE transaction, via `crud::retire_run`. Ordering alone is not enough:
/// cancelling the rows removes the run from `find_pending_global_runs` (whose
/// selection requires a `queued`, non-scope-owned row), so a terminal write
/// that then failed would strand the run non-terminal and unselectable, with
/// nothing left to retry it. Rolling both back leaves the run exactly as it was
/// and the next tick retries the whole step.
async fn retire_exhausted_run(db: &DatabaseConnection, run_id: &str, attempt: i32) {
    let reason = format!("exceeded {MAX_RECOVERY_ATTEMPTS} recovery attempts");
    match agentic_runtime::crud::retire_run(db, run_id, &reason).await {
        Ok(()) => tracing::error!(
            target: "recovery",
            run_id = %run_id,
            attempt,
            max = MAX_RECOVERY_ATTEMPTS,
            "retired a run that exhausted its recovery budget"
        ),
        Err(e) => tracing::error!(
            target: "recovery",
            run_id = %run_id,
            attempt,
            error = %e,
            "recovery budget exhausted but the run could not be retired; \
             rolled back, retrying next tick"
        ),
    }
}

/// The body of [`recover_single_run`] that runs once the driver lease is held.
///
/// Exists so ownership is a call boundary: everything here executes as the
/// run's owner, which is what makes releasing run-scoped resources on failure
/// sound.
#[allow(clippy::too_many_arguments)]
async fn recover_single_run_owned(
    root: &agentic_runtime::entity::run::Model,
    db: DatabaseConnection,
    state: Arc<RuntimeState>,
    platform: Arc<dyn PlatformContext>,
    builder_bridges: Option<BuilderBridges>,
    schema_cache: Option<Arc<Mutex<HashMap<String, agentic_analytics::SchemaCatalog>>>>,
    builder_test_runner: Option<Arc<dyn agentic_builder::BuilderTestRunner>>,
    builder_app_runner: Option<Arc<dyn agentic_builder::BuilderAppRunner>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    custom_executors: Option<Arc<agentic_runtime::worker::CustomTaskRegistry>>,
    // The lease this frame owns — the heartbeat ticker downstream renews it.
    driver_id: String,
) -> Result<(), String> {
    use agentic_core::transport::{CoordinatorTransport, WorkerTransport};

    // Scope to this run's task tree so recovery's worker can't poach
    // a sibling run's queued root. See `drive_with_coordinator` in
    // `lib.rs` for the full explanation of why this matters under
    // LISTEN/NOTIFY-driven wake.
    let transport = DurableTransport::with_router(db.clone(), router, Some(root.id.clone()));
    // A preview run gets no builder: its bridges edit the working copy, and an
    // analytics agent can delegate to it. Without bridges the delegation fails
    // instead of writing a file.
    let is_preview = crate::platform::preview::is_preview(platform.as_ref());
    let executor = Arc::new(PipelineTaskExecutor {
        platform,
        builder_bridges: builder_bridges.filter(|_| !is_preview),
        schema_cache,
        builder_test_runner: builder_test_runner.filter(|_| !is_preview),
        builder_app_runner: builder_app_runner.filter(|_| !is_preview),
        db: db.clone(),
        state: Some(state.clone()),
        custom_executors,
    });

    // ── Step 0: Transparent recovery — clean up partial events ──────────
    //
    // Recovery is transparent: no attempt increment, no attempt_started event.
    // Instead, delete partial events from the interrupted execution (e.g. a
    // step_started without its step_end) and emit a lightweight recovery_resumed
    // marker. This prevents duplicate events in the frontend reasoning trace.
    //
    // Skip the marker entirely for runs that have NEVER executed — a
    // freshly-seeded Global run (scheduler tick / run-now) hits the
    // latency worker before its first event, and "Resuming from server
    // restart" is misleading for a run that's about to start for the
    // first time.
    let attempt = root.attempt; // Same attempt — no increment
    let prior_event_count = agentic_runtime::crud::get_max_seq(&db, &root.id)
        .await
        .unwrap_or(-1)
        + 1;
    let is_fresh_seed = prior_event_count == 0;
    tracing::debug!(target: "recovery", run_id = %root.id, attempt, prior_event_count, "starting transparent recovery");

    // Delete partial events: find the last completed boundary and remove
    // everything after it. This cleans up step_started events that were
    // emitted before the crash but never got their corresponding step_end.
    if !is_fresh_seed {
        let all_events = agentic_runtime::crud::get_all_events(&db, &root.id)
            .await
            .unwrap_or_default();
        if let Some(last_complete) = all_events.iter().rev().find(|e| {
            matches!(
                e.event_type.as_str(),
                "step_end"
                    | "done"
                    | "error"
                    | "cancelled"
                    | "subrun_completed"
                    | "subrun_step_completed"
            )
        }) {
            let delete_from = last_complete.seq + 1;
            if delete_from <= all_events.last().map(|e| e.seq).unwrap_or(0) {
                tracing::info!(
                    target: "recovery",
                    run_id = %root.id,
                    from_seq = delete_from,
                    "deleting partial events from interrupted execution"
                );
                agentic_runtime::crud::delete_events_from_seq(&db, &root.id, delete_from)
                    .await
                    .ok();
            }
        }

        // Emit recovery marker on the root run (same attempt number) —
        // only for runs that actually have prior events to resume from.
        let next_seq = agentic_runtime::crud::get_max_seq(&db, &root.id)
            .await
            .unwrap_or(-1)
            + 1;
        agentic_runtime::crud::insert_event(
            &db,
            &root.id,
            next_seq,
            "recovery_resumed",
            &serde_json::json!({"message": "Resuming from server restart"}),
            attempt,
        )
        .await
        .ok();
    }

    // Also emit recovery_resumed on non-terminal child runs so their SSE
    // streams (e.g. builder delegation panel) close interrupted steps.
    {
        let child_tree = agentic_runtime::crud::load_task_tree(&db, &root.id)
            .await
            .unwrap_or_default();
        for child in &child_tree {
            if child.id == root.id {
                continue;
            }
            if matches!(
                child.task_status.as_deref(),
                Some("done") | Some("failed") | Some("cancelled")
            ) {
                continue;
            }
            let child_seq = agentic_runtime::crud::get_max_seq(&db, &child.id)
                .await
                .unwrap_or(-1)
                + 1;
            agentic_runtime::crud::insert_event(
                &db,
                &child.id,
                child_seq,
                "recovery_resumed",
                &serde_json::json!({"message": "Resuming from server restart"}),
                attempt,
            )
            .await
            .ok();
        }
    }

    // ── Step 1: Reconstruct coordinator from DB ─────────────────────────
    let (coordinator, pending_resumes) = Coordinator::from_db(
        db.clone(),
        state.clone(),
        transport.clone() as Arc<dyn CoordinatorTransport>,
        &root.id,
    )
    .await
    .map_err(|e| format!("failed to reconstruct coordinator: {e}"))?;
    // `from_db` returns a coordinator with the default no-op
    // completion policy + resolver — recovered runs may still
    // complete with `workflow_continue` metadata and may still
    // suspend on automation delegations, so re-attach both the
    // automation policy and resolver before driving.
    let coordinator = coordinator
        .with_completion_policy(Arc::new(agentic_automation::AutomationCompletionPolicy))
        .with_delegation_resolver(Arc::new(agentic_automation::AutomationDelegationResolver));

    // ── Step 2: Walk tree and classify each task ────────────────────────
    let tree = agentic_runtime::crud::load_task_tree(&db, &root.id)
        .await
        .map_err(|e| format!("failed to load task tree: {e}"))?;

    // A root that has reached a suspension continues from it — resumed below,
    // or left parked. Its own queue entry must agree before the worker in
    // step 4 exists: a `queued` entry that predates the suspension would start
    // the run again from the top, and one that *is* the resume must not be
    // doubled by a re-launch here. See `root_entry`.
    let root_entry = root_entry::reconcile(&db, &transport, &root.id).await?;

    // A parent whose children have all reported is continued by the resume
    // that carries their answer — the one the coordinator assigns in step 4,
    // or the one already on the queue, which it is only told about — and by
    // nothing else. Re-launching it here as well was a second continuation of
    // the run, with an empty answer. See `children_done`.
    let children_done = children_done::settle(&root.id, root_entry, pending_resumes);

    for task_run in &tree {
        match task_run.task_status.as_deref() {
            Some("done") | Some("failed") => continue,

            Some("awaiting_input") => {
                tracing::debug!(target: "recovery", task_id = %task_run.id, "leaving HITL-suspended task");
                continue;
            }

            // Whatever status the row was left in: `delegating` when a queued
            // run is picked up, `needs_resume` once a boot has stamped it.
            _ if children_done.resumes(&task_run.id) => {
                tracing::debug!(
                    target: "recovery",
                    task_id = %task_run.id,
                    "children have all reported; continued by the resume that carries their answer"
                );
            }

            Some("delegating") => {
                if children_done.cannot_resume(&task_run.id) {
                    // Every child in and no checkpoint: nothing can hand the
                    // parent its answer. The re-launch says so — an analytics
                    // or builder run has nothing to resume from — and that
                    // closes the run as interrupted instead of leaving it
                    // parked until the suspend ceiling.
                    re_launch_task(&db, &state, &executor, &transport, task_run).await?;
                } else {
                    tracing::debug!(target: "recovery", task_id = %task_run.id, "parent still waiting");
                }
            }

            _ => {
                // running / needs_resume / shutdown / unknown

                // Check if this task has non-terminal children. If so, it was
                // delegating before the crash and the reaper changed its status.
                // Don't re-launch it — the coordinator's WaitingOnChildren state
                // handles it; children complete → coordinator resumes this parent.
                let has_active_children = tree.iter().any(|t| {
                    t.parent_run_id.as_deref() == Some(task_run.id.as_str())
                        && !matches!(t.task_status.as_deref(), Some("done") | Some("failed"))
                });
                if has_active_children {
                    // Restore the correct DB status — this task was delegating
                    // before the crash but the reaper set it to needs_resume.
                    agentic_runtime::crud::update_task_status(
                        &db,
                        &task_run.id,
                        "delegating",
                        None,
                    )
                    .await
                    .ok();
                    tracing::info!(
                        target: "recovery",
                        task_id = %task_run.id,
                        "skipping re-launch: has active children (restored to delegating)"
                    );
                    continue;
                }

                let suspend_data = agentic_runtime::crud::get_suspension(&db, &task_run.id)
                    .await
                    .ok()
                    .flatten();

                // Root task gets the same checkpoint-presence check as
                // children. The old "always re-launch root" rule was wrong
                // for freshly-seeded Global runs (scheduler tick / run-now):
                // they have no suspension, no workflow_state, no
                // task_metadata.original_spec — `resume_from_state` errors
                // out with "no saved state" and the run is force-failed.
                // For those, the queue entry for the root is still
                // `queued`; the Worker spawned at step 4 below will claim
                // it and execute the spec fresh. Same logic for
                // reaper-requeued roots — the worker re-executes
                // idempotently.
                let is_root_with_queued_entry = task_run.id == root.id
                    && matches!(
                        agentic_runtime::crud::get_queue_entry(&db, &task_run.id).await,
                        Ok(Some(q)) if q.queue_status == "queued"
                    );

                // Only ever the root, and only when its entry is the resume
                // from the suspension it is at (a resumed root is `running`,
                // so this arm is the one that sees it).
                let entry_resumes_it = task_run.id == root.id && root_entry == RootEntry::Continues;

                if suspend_data.is_some() && entry_resumes_it {
                    tracing::debug!(
                        target: "recovery",
                        run_id = %task_run.id,
                        "root's queued entry resumes it from this suspension; worker will claim it"
                    );
                } else if suspend_data.is_some() {
                    re_launch_task(&db, &state, &executor, &transport, task_run).await?;
                } else if is_root_with_queued_entry {
                    tracing::debug!(
                        target: "recovery",
                        run_id = %task_run.id,
                        "root has no checkpoint and queue entry is queued; worker will claim and execute fresh"
                    );
                } else if task_run.id == root.id {
                    // Root with no suspension AND no queued entry — the
                    // queue row was either claimed-and-orphaned or never
                    // enqueued. Fall back to the legacy re_launch path so
                    // the existing crash-recovery tests still pass; that
                    // path will surface a clear error if there's truly
                    // nothing to resume.
                    re_launch_task(&db, &state, &executor, &transport, task_run).await?;
                } else if let Some(spec) = extract_original_spec(task_run) {
                    // Child task was running with no checkpoint but has an original
                    // TaskSpec (stored on creation). Re-enqueue it — the worker will
                    // re-execute from scratch (idempotent, like Temporal activity retry).
                    tracing::info!(
                        target: "recovery",
                        task_id = %task_run.id,
                        source_type = ?task_run.source_type,
                        "re-enqueueing checkpointless child task from original spec"
                    );
                    reenqueue_child(&db, &transport, task_run, spec).await?;
                } else {
                    tracing::debug!(target: "recovery", task_id = %task_run.id, "no checkpoint and no original spec, marking failed");
                    fail_stale_child(&db, task_run).await;
                }
            }
        }
    }

    // ── Step 3: Process pending resumes ─────────────────────────────────
    //
    // For Temporal-style automation runs, the coordinator's resume_parent will
    // enqueue an AutomationDecision task when it processes these resumes — no
    // in-memory channel needed. For analytics/builder runs, resume_parent
    // assigns a TaskSpec::Resume which the worker handles.
    //
    // The coordinator processes them when it starts, in step 4. They are the
    // only continuation of those parents: the walk above skipped every one.

    // ── Step 4: Register in RuntimeState + spawn coordinator + worker ───
    // Without registration the SSE endpoint finds no notifier and exits
    // immediately, so recovered runs appear "dead" to connected clients.
    let cancel_rx = {
        let (answer_tx, _answer_rx) = tokio::sync::mpsc::channel::<String>(1);
        let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
        state.register(&root.id, answer_tx, cancel_tx);
        cancel_rx
    };

    // Cross-process cancel forwarder (§12 FU4a). A recovered / Global run
    // is driven here, out-of-process from the API that handles the cancel
    // request — so the in-memory `state.cancel` watch may have no listener
    // in this process. Observe BOTH the in-proc watch (same-process fast
    // path) and the durable `cancel_requested_at` DB flag (cross-process),
    // and tear down the subtree on either. Self-terminates once the run is
    // terminal so it doesn't poll forever.
    {
        let cf_transport = transport.clone();
        let cf_db = db.clone();
        let cf_run = root.id.clone();
        let mut cf_rx = cancel_rx;
        spawn_with_hub(async move {
            let poll = std::time::Duration::from_secs(5);
            loop {
                let cancelled = tokio::select! {
                    changed = cf_rx.changed() => changed.is_ok() && *cf_rx.borrow(),
                    _ = tokio::time::sleep(poll) => {
                        agentic_runtime::crud::is_cancel_requested(&cf_db, &cf_run)
                            .await
                            .unwrap_or(false)
                    }
                };
                if cancelled {
                    tracing::info!(
                        target: "recovery",
                        run_id = %cf_run,
                        "cancel observed (cross-process); cancelling subtree"
                    );
                    let _ = cf_transport.cancel_subtree(&cf_run).await;
                    break;
                }
                if let Ok(Some(r)) = agentic_runtime::crud::get_run(&cf_db, &cf_run).await
                    && matches!(
                        r.task_status.as_deref(),
                        Some("done") | Some("failed") | Some("cancelled") | Some("timed_out")
                    )
                {
                    break;
                }
            }
        });
    }
    // Register notifiers for non-terminal child runs so their SSE streams
    // work after recovery. Without this, the frontend opens an SSE connection
    // for a recovered child (e.g. builder delegation) and gets no notifier →
    // the stream exits immediately.
    for task_run in &tree {
        if task_run.id == root.id {
            continue; // Already registered above.
        }
        if matches!(
            task_run.task_status.as_deref(),
            Some("done") | Some("failed") | Some("cancelled")
        ) {
            continue;
        }
        state.register_notifier(&task_run.id);
    }

    // Heartbeat the driver lease while this run is being driven. Closes the
    // Task 5 seam: without this, a run that takes longer than
    // DRIVER_LEASE_TTL_SECS would have its lease go stale mid-flight and a
    // concurrent recovery tick (Task 6) could double-drive it. The task
    // self-terminates the moment the lease is no longer ours — either it
    // was stolen (heartbeat returns false) or the run reached a terminal
    // state (terminal `transition_run` nulls `driver_id`, so the next
    // heartbeat returns false). On process exit the runtime is torn down;
    // the lease then goes stale within the TTL and is reclaimable, which is
    // exactly the desired crash/restart behavior.
    {
        // TTL / 3 — three missed heartbeats before another driver may steal.
        let interval = std::time::Duration::from_secs(
            (agentic_runtime::crud::DRIVER_LEASE_TTL_SECS / 3).max(1) as u64,
        );
        let hb_db = db.clone();
        let hb_run_id = root.id.clone();
        let hb_driver_id = driver_id.clone();
        spawn_with_hub(async move {
            loop {
                tokio::time::sleep(interval).await;
                match agentic_runtime::crud::heartbeat_driver(&hb_db, &hb_run_id, &hb_driver_id)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => break, // lost the lease or run terminal
                    Err(e) => {
                        tracing::warn!(
                            target: "recovery",
                            run_id = %hb_run_id,
                            error = %e,
                            "driver heartbeat failed; retrying next tick"
                        );
                    }
                }
            }
        });
    }

    let worker = Worker::new(transport.clone() as Arc<dyn WorkerTransport>, executor);
    spawn_with_hub(async move { worker.run().await });

    let pending_resumes = children_done.for_coordinator;
    let pending_count = pending_resumes.len();
    let retire_transport = transport.clone();
    spawn_with_hub(async move {
        let mut coord = coordinator;
        coord.process_pending_resumes(pending_resumes).await;
        coord.run().await;
        // `coord.run()` returns only once every task in this tree is terminal,
        // so nothing can legitimately be claimed under this root again. Retire
        // the workers built on this transport instead of leaving them polling
        // the queue for the life of the process — see
        // `DurableTransport::retire_worker_loop`.
        retire_transport.retire_worker_loop();
    });

    tracing::debug!(
        target: "recovery",
        run_id = %root.id,
        tree_size = tree.len(),
        pending = pending_count,
        "recovery complete"
    );

    Ok(())
}

/// Re-launch a single task from its saved state.
async fn re_launch_task(
    db: &DatabaseConnection,
    _state: &Arc<RuntimeState>,
    executor: &Arc<PipelineTaskExecutor>,
    transport: &Arc<DurableTransport>,
    task_run: &agentic_runtime::entity::run::Model,
) -> Result<(), String> {
    use agentic_core::transport::WorkerTransport;
    use agentic_runtime::worker::TaskExecutor;

    let suspend_data = agentic_runtime::crud::get_suspension(db, &task_run.id)
        .await
        .ok()
        .flatten();

    let executing = executor
        .resume_from_state(task_run, suspend_data)
        .await
        .map_err(|e| format!("failed to resume task {}: {e}", task_run.id))?;

    agentic_runtime::crud::update_run_running(db, &task_run.id)
        .await
        .ok();
    agentic_runtime::crud::update_task_status(db, &task_run.id, "running", None)
        .await
        .ok();

    spawn_virtual_worker(
        transport.clone() as Arc<dyn WorkerTransport>,
        &task_run.id,
        executing,
    );

    tracing::info!(
        target: "recovery",
        task_id = %task_run.id,
        source_type = ?task_run.source_type,
        "re-launched task"
    );

    Ok(())
}

/// Mark a stale child as failed and write an outcome for its parent.
async fn fail_stale_child(db: &DatabaseConnection, task_run: &agentic_runtime::entity::run::Model) {
    agentic_runtime::crud::mark_recovery_failed(
        db,
        &task_run.id,
        "stale child; parent will re-delegate",
    )
    .await
    .ok();

    if let Some(ref parent_id) = task_run.parent_run_id {
        agentic_runtime::crud::insert_task_outcome(
            db,
            &task_run.id,
            parent_id,
            "failed",
            Some("stale child; parent will re-delegate"),
        )
        .await
        .ok();
    }
}

/// Extract the original TaskSpec from a child run's task_metadata.
///
/// The coordinator stores `original_spec` in task_metadata when spawning children
/// (for retry/fallback). We reuse it here to re-enqueue checkpointless tasks.
fn extract_original_spec(
    task_run: &agentic_runtime::entity::run::Model,
) -> Option<agentic_core::delegation::TaskSpec> {
    let meta = task_run.task_metadata.as_ref()?;
    let spec_val = meta.get("original_spec")?;
    serde_json::from_value(spec_val.clone()).ok()
}

/// Re-enqueue a child task through the durable queue using its original TaskSpec.
///
/// The task gets a fresh execution — the worker will pick it up and run it from
/// scratch. This is the Temporal-style "activity retry" pattern: the task is
/// idempotent, so re-running it produces the correct result.
async fn reenqueue_child(
    db: &sea_orm::DatabaseConnection,
    transport: &std::sync::Arc<DurableTransport>,
    task_run: &agentic_runtime::entity::run::Model,
    spec: agentic_core::delegation::TaskSpec,
) -> Result<(), String> {
    // Reset task_status to running so the coordinator tracks it correctly.
    agentic_runtime::crud::transition_run(db, &task_run.id, "running", None, None, None)
        .await
        .ok();

    // Use requeue_task (upsert) instead of enqueue_task (insert) — the queue
    // row already exists from the original execution and would cause a PK
    // violation on insert.
    agentic_runtime::crud::requeue_task(db, &task_run.id, &spec)
        .await
        .map_err(|e| format!("failed to requeue child {}: {e}", task_run.id))?;

    // Wake the worker so it picks up the re-queued task immediately.
    transport.notify_new_task();

    Ok(())
}

/// Forward an ExecutingTask's events/outcomes to the coordinator via transport.
fn spawn_virtual_worker(
    transport: Arc<dyn agentic_core::transport::WorkerTransport>,
    task_id: &str,
    executing: agentic_runtime::worker::ExecutingTask,
) {
    use agentic_core::delegation::TaskOutcome;
    use agentic_core::transport::WorkerMessage;

    let task_id = task_id.to_string();
    let transport_clone = transport.clone();
    let task_id_clone = task_id.clone();

    // Spawn heartbeat loop for the recovered task.
    let heartbeat_cancel = transport.spawn_heartbeat(
        &task_id,
        agentic_runtime::orchestrator::worker::HEARTBEAT_INTERVAL,
    );

    spawn_with_hub(async move {
        let mut events = executing.events;
        while let Some((event_type, payload)) = events.recv().await {
            if transport_clone
                .send(WorkerMessage::Event {
                    task_id: task_id_clone.clone(),
                    event_type,
                    payload,
                })
                .await
                .is_err()
            {
                break;
            }
        }
    });

    let task_id_for_outcomes = task_id;
    spawn_with_hub(async move {
        let mut outcomes = executing.outcomes;
        // Whether the driver stopped while still holding the claim — see
        // `Worker::handle_task`'s cleanup block. This third driver has to
        // honour the same rule as the other two.
        let mut parked_suspended = false;
        while let Some(outcome) = outcomes.recv().await {
            let is_terminal = matches!(
                outcome,
                TaskOutcome::Done { .. } | TaskOutcome::Failed(_) | TaskOutcome::Cancelled
            );
            parked_suspended = matches!(outcome, TaskOutcome::Suspended { .. });
            // Captured before the send moves `outcome` — it is the field that
            // tells an operator whether the dropped outcome was the `Suspended`
            // that strands a claim or a terminal one that doesn't.
            let outcome_type = match &outcome {
                TaskOutcome::Done { .. } => "Done",
                TaskOutcome::Suspended { .. } => "Suspended",
                TaskOutcome::Failed(_) => "Failed",
                TaskOutcome::Cancelled => "Cancelled",
                // NOT `unreachable!` here, unlike the other two drivers — this
                // one has no `Defer` translation, so a `Deferred` would be
                // forwarded as a plain `Outcome` and dropped by
                // `Coordinator::handle_outcome`, which documents that variant as
                // unreachable by construction.
                //
                // It cannot arrive today, and the durable way to say why is the
                // *builder*, not the route list: `deferred_task` is constructed
                // only inside `execute_airway`, and no path out of
                // `resume_from_state` reaches that function — so neither producer
                // of `Deferred` (unresolvable pipeline YAML, single-flight lease
                // held) is reachable from a re-launch. Stated this way the claim
                // survives a fourth resume route being added; enumerating the
                // routes would not, and an earlier draft of this comment had
                // already missed one (`execute_automation`, via the
                // `task_metadata.original_spec` branch).
                //
                // If a resume path ever does reach airway admission, note the
                // failure is quiet rather than loud: the deferral is dropped, the
                // ticker is cancelled, and the row comes back only via the reaper
                // — which charges a `claim_count` that a real `Defer`
                // deliberately refunds.
                TaskOutcome::Deferred { .. } => "Deferred",
            };
            // Same rule as the other two drivers: a dropped `Suspended` would
            // park the claim on a ticker for an outcome no coordinator ever
            // received, leaving the row `claimed` with every backstop disarmed.
            // Let the claim go stale instead so the reaper → `find_stuck_runs`
            // chain takes over. This driver has one backstop FEWER than
            // `Worker::handle_task` — no `saw_any_outcome` synthesized `Failed`
            // — so it needs this at least as much.
            if let Err(e) = transport
                .send(WorkerMessage::Outcome {
                    task_id: task_id_for_outcomes.clone(),
                    outcome,
                })
                .await
            {
                // `target: "recovery"` like every other line in this file — an
                // operator narrowing to the recovery sweep must not lose the one
                // line saying a recovered task's outcome went nowhere. The
                // three-driver rule is filterable via `rule` instead, which
                // works across targets where a shared target would not.
                tracing::error!(
                    target: "recovery",
                    rule = "dropped-outcome",
                    task_id = %task_id_for_outcomes,
                    outcome_type,
                    error = %e,
                    "failed to deliver outcome to the coordinator"
                );
                parked_suspended = false;
            }
            if is_terminal {
                break;
            }
        }
        if !parked_suspended {
            heartbeat_cancel.cancel();
        }
    });
}

#[cfg(test)]
mod recovery_budget_tests {
    use super::*;

    #[test]
    fn a_run_is_driven_exactly_max_recovery_attempts_times() {
        // `increment_attempt` returns the NEW value and the column starts at 0,
        // so the Nth recovery observes N. Four drives, then retirement.
        for attempt in 1..=MAX_RECOVERY_ATTEMPTS {
            assert!(
                !recovery_budget_exhausted(attempt),
                "attempt {attempt} must still be allowed to drive"
            );
        }
        assert!(
            recovery_budget_exhausted(MAX_RECOVERY_ATTEMPTS + 1),
            "the drive after the budget must retire the run"
        );
    }

    #[test]
    fn budget_is_four_and_bounded_at_both_ends() {
        // Pinned deliberately: this number is the whole guard. Raising it is a
        // decision, not a refactor.
        assert_eq!(MAX_RECOVERY_ATTEMPTS, 4);
        // A fresh run (never recovered) is never retired by this check...
        assert!(!recovery_budget_exhausted(0));
        // ...and no value above the budget escapes it.
        assert!(recovery_budget_exhausted(i32::MAX));
    }
}

#[cfg(test)]
mod unresumable_tests {
    use super::is_unresumable;
    use crate::executor::{NO_CHECKPOINT, NO_SAVED_STATE};

    /// The shapes recovery actually receives: the executor's message wrapped by
    /// the resume path, as it arrived in Sentry on 2026-09-15.
    #[test]
    fn a_run_interrupted_before_its_first_checkpoint_is_unresumable() {
        let wrapped =
            format!("failed to resume task t1: run t1 (type=app_function) {NO_CHECKPOINT}");
        assert!(is_unresumable(&wrapped));
        assert!(is_unresumable(&format!(
            "cannot resume automation run r1: {NO_SAVED_STATE}"
        )));
    }

    /// Everything else is a recovery that broke and must stay loud.
    #[test]
    fn any_other_recovery_failure_is_not_unresumable() {
        assert!(!is_unresumable("database connection closed"));
        assert!(!is_unresumable(
            "failed to resume task t1: coordinator panicked"
        ));
    }
}
