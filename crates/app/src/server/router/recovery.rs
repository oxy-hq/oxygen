//! Background startup recovery for in-flight agentic runs.
//!
//! [`cleanup_stale_runs`] (called from [`super::entry::new_agentic_state`])
//! only transitions interrupted runs to `task_status = "needs_resume"`. The
//! runs themselves are not re-driven until [`recover_active_runs`] rebuilds a
//! `PlatformContext` + `BuilderBridges` per workspace, and hands them to the
//! pipeline's recovery entry point.
//!
//! Spawned in the background so the HTTP listener can start binding while
//! recovery is still in progress — a stuck workspace should not keep the
//! server from serving healthchecks.

use std::sync::Arc;

use agentic_http::AgenticState;
use agentic_pipeline::BuilderTestRunnerTrait;
use agentic_pipeline::platform::{BuilderBridges, PlatformContext};
use agentic_pipeline::recovery::{recover_active_runs, recover_stranded_runs};
use agentic_runtime::state::RuntimeState;
use oxy::adapters::secrets::SecretsManager;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use sea_orm::{DatabaseConnection, EntityTrait};

use crate::agentic_wiring::{OxyProjectContext, build_builder_bridges};
use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
use crate::server::queued_analytics::caller::CallerRunResolver;
use crate::server::service::secret_manager::SecretManagerService;

use super::seams::TaskSeam;

/// The custom-app task kinds the composition root handed in, shared by every
/// driver loop this process runs.
pub(super) type CustomTasks = Arc<[TaskSeam]>;
use oxy::config::WorkingCopy;
use oxy_app_core::serve_mode::{LOCAL_WORKSPACE_ID, ServeMode};

/// Handles for the hooks spawned by [`spawn_shutdown_hook`], so the serve
/// command can wait for them before the process exits.
///
/// A process-lifetime registry rather than a value threaded through the
/// router return type, deliberately: `api_router` already returns a tuple
/// consumed at nine call sites (most of them tests), and
/// `serve_application` (`oxy-server`'s `commands/serve.rs`) — the only place that
/// owns the shutdown wait — never sees the `AgenticState` the hook is built
/// from. Threading a third element through would be six signature changes to
/// move a value whose lifetime is the process anyway, which is exactly what
/// the shutdown token itself already is.
static SHUTDOWN_HOOKS: std::sync::Mutex<Vec<tokio::task::JoinHandle<()>>> =
    std::sync::Mutex::new(Vec::new());

/// How long [`await_shutdown_hooks`] waits before giving up on the hooks.
///
/// Well inside a default Kubernetes 30s termination grace period, so a slow
/// or wedged database can never be the reason a pod gets SIGKILLed.
const SHUTDOWN_HOOK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// How long the shutdown hook waits for cancelled direct drives to release
/// their driver leases. Well inside [`SHUTDOWN_HOOK_TIMEOUT`], which also has
/// to cover the queue releases after it.
const DIRECT_DRIVE_UNWIND_WAIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Spawn the graceful-shutdown hook. Returns immediately.
///
/// When `agentic_state.shutdown_token` is cancelled (the serve command
/// forwards SIGINT/SIGTERM into it), mark every active run as `"shutdown"`
/// and signal their cancel channels, then release this process's durable
/// queue claims. Runs with `task_status = "shutdown"` are picked up by
/// [`recover_active_runs`] on the next startup, so this is the resumable
/// counterpart to the `"cancelled"` state set by user-initiated cancel.
///
/// The claim release happens here, in `oxy-app`, rather than inside
/// `RuntimeState::shutdown_all` — `agentic-runtime`'s `lifecycle/` sub-layer
/// (which owns `shutdown_all`) must have zero deps on its own `orchestrator/`
/// sub-layer (see `crates/agentic/runtime/CLAUDE.md`), but `oxy-app` may
/// legitimately import both, so this is where the two halves of "shut this
/// process down" are stitched back together.
///
/// The handle is registered in [`SHUTDOWN_HOOKS`]; call
/// [`await_shutdown_hooks`] on the way out of `serve` so the release
/// actually lands instead of racing process exit.
pub(super) fn spawn_shutdown_hook(agentic_state: Arc<AgenticState>) {
    let token = agentic_state.shutdown_token.clone();
    let runtime = agentic_state.runtime.clone();
    let db = agentic_state.db.clone();
    let handle = tokio::spawn(async move {
        token.cancelled().await;

        // Close the claim path FIRST, before anything else touches the queue.
        // The release below flips rows `claimed -> queued`, which fires the
        // NOTIFY trigger and wakes this process's own workers; without this
        // they would re-claim what we just released and die holding it. This
        // is the only ordering guarantee here, and it is one we establish
        // ourselves rather than inherit from `shutdown_all`.
        agentic_runtime::transport::begin_shutdown();

        let count = runtime.shutdown_all(&db).await;
        if count > 0 {
            tracing::info!(
                target: "recovery",
                count,
                "graceful shutdown: marked active runs resumable"
            );
        }
        // Give the cancelled direct drives a moment to unwind: each releases
        // its own driver lease on the way out, so the successor's first
        // startup pass resumes those `shutdown` runs at boot. A drive still
        // running past the wait keeps its lease (it lapses within the TTL)
        // — releasing it here would let a peer re-drive a live run.
        let still_held = agentic_runtime::lifecycle::driver_lease::await_held_leases_released(
            DIRECT_DRIVE_UNWIND_WAIT,
        )
        .await;
        if still_held > 0 {
            tracing::info!(
                target: "recovery",
                still_held,
                "graceful shutdown: direct drives still unwinding; their leases lapse within the TTL"
            );
        }

        // Give back every durable-queue claim this process holds so a
        // successor can pick the work up immediately, budget-neutral.
        //
        // Note what this is NOT: `shutdown_all` fires cancel channels
        // fire-and-forget (`tx.send(true).ok()`, no join), so a task that was
        // mid-execution may still be running when its row goes back to
        // `queued`. There is no completion barrier here, and the correctness
        // of that is owned elsewhere — the `try_acquire_driver` CAS prevents
        // double-drive, and the heartbeat's `worker_id`/`claimed` predicate
        // stops a still-ticking heartbeat from re-stamping a row we no longer
        // hold. What the `begin_shutdown` above guarantees is narrower and
        // sufficient: nothing in this process claims *new* work from here on.
        //
        // `process_worker_id_if_initialized` rather than `process_worker_id`:
        // the id is lazily minted, so asking for it here in a process that
        // never built a `DurableTransport` would forge a fresh id and issue a
        // guaranteed-zero-row UPDATE. `None` means we never claimed anything.
        let Some(worker_id) = agentic_runtime::transport::process_worker_id_if_initialized() else {
            return;
        };

        // Order matters: `mark_released_roots_global` matches on this
        // worker's `worker_id` + `queue_status = 'claimed'`, and the drain
        // below clears both (`worker_id -> NULL`, status -> `queued`). Run it
        // first so an orphaned workflow/airway root becomes visible to the
        // global claim path instead of waiting for a process restart — see
        // `mark_released_roots_global`'s doc comment for why this is gated to
        // `workflow`/`airway` and roots only.
        match agentic_runtime::crud::mark_released_roots_global(&db, worker_id).await {
            Ok(0) => {}
            Ok(marked) => tracing::info!(
                target: "recovery",
                marked,
                worker_id,
                "graceful shutdown: made orphaned roots globally recoverable"
            ),
            Err(e) => tracing::warn!(
                target: "recovery",
                error = %e,
                worker_id,
                "graceful shutdown: failed to mark roots global"
            ),
        }

        // `drain_claims_for_worker` rather than a single release pass: a worker
        // that cleared the `is_shutting_down` gate before it was set can still
        // land its claim *after* the release runs, and the `shutdown_all` above
        // does one DB write per active run in between. The drain is bounded (3
        // passes, stopping on the first empty one) so it stays inside
        // `SHUTDOWN_HOOK_TIMEOUT`, which still wraps this whole task.
        match agentic_runtime::crud::drain_claims_for_worker(&db, worker_id).await {
            Ok(0) => {}
            Ok(released) => tracing::info!(
                target: "recovery",
                released,
                worker_id,
                "graceful shutdown: released claims back to the queue"
            ),
            Err(e) => tracing::warn!(
                target: "recovery",
                error = %e,
                worker_id,
                "graceful shutdown: failed to release claims; the reaper will \
                 reclaim them after the visibility timeout"
            ),
        }
    });
    // A poisoned lock only means some other thread panicked while holding a
    // `Vec<JoinHandle>`; the Vec is still structurally sound, so recover
    // rather than turn a shutdown-path detail into a panic.
    SHUTDOWN_HOOKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(handle);
}

/// Register a task the serve command should wait for (bounded) before the
/// process exits — work that must happen once the shutdown token is cancelled,
/// like the final token-usage flush.
pub(crate) fn register_shutdown_hook(handle: tokio::task::JoinHandle<()>) {
    SHUTDOWN_HOOKS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(handle);
}

/// Wait for every registered shutdown hook to finish, bounded by
/// [`SHUTDOWN_HOOK_TIMEOUT`].
///
/// The hooks are detached tasks racing process exit; without this the claim
/// release in `shutdown_all` is a coin flip. Bounded because a slow database
/// must not hold the pod past its termination grace period — on timeout we
/// degrade to the pre-existing behaviour, where the reaper reclaims the
/// claims once the visibility timeout expires.
///
/// `shutdown_token` is the one the hooks are parked on. If it was never
/// cancelled the server is unwinding from an error rather than a signal, so
/// the hooks will never fire and waiting on them would just burn the full
/// timeout and log a shutdown warning for a failure that isn't one.
pub async fn await_shutdown_hooks(shutdown_token: &tokio_util::sync::CancellationToken) {
    if !shutdown_token.is_cancelled() {
        return;
    }

    let handles: Vec<_> =
        std::mem::take(&mut *SHUTDOWN_HOOKS.lock().unwrap_or_else(|e| e.into_inner()));
    if handles.is_empty() {
        return;
    }

    let drain = async {
        for handle in handles {
            // A hook that panicked has already logged; the remaining hooks
            // still deserve their chance to release.
            let _ = handle.await;
        }
    };

    if tokio::time::timeout(SHUTDOWN_HOOK_TIMEOUT, drain)
        .await
        .is_err()
    {
        tracing::warn!(
            target: "recovery",
            timeout_secs = SHUTDOWN_HOOK_TIMEOUT.as_secs(),
            "graceful shutdown: claim release timed out; the reaper will reclaim"
        );
    }
}

/// The in-process global driver loop: after one-shot startup recovery, re-run
/// recovery on an interval so scheduler-seeded (Phase 2) and crash-orphaned
/// `scope_owned = false` runs are driven to completion without a per-request
/// coordinator. It also fires the periodic schedule + monitor-scan ticks
/// (`tick_schedules` / `tick_monitor_schedules`).
///
/// Enabled by ROLE, not a standalone flag: every role except the stateless
/// `serve` replica runs it (`all` / `ide` / `worker`). `OXY_INPROC_GLOBAL_WORKER`
/// remains an override in both directions (`=0` forces it off on a non-serve
/// node). Firing is exactly-once across replicas via the scheduler `next_run_at`
/// CAS, so several eligible nodes running it concurrently is safe — no leader
/// election needed.
pub const INPROC_GLOBAL_WORKER_ENV: &str = "OXY_INPROC_GLOBAL_WORKER";
const INPROC_GLOBAL_WORKER_INTERVAL_ENV: &str = "OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS";
pub const DEFAULT_INPROC_GLOBAL_WORKER_INTERVAL_SECS: u64 = 30;

pub fn inproc_global_worker_enabled() -> bool {
    // Explicit env override wins, in both directions.
    if let Ok(v) = std::env::var(INPROC_GLOBAL_WORKER_ENV) {
        return matches!(v.as_str(), "1" | "true" | "yes" | "on");
    }
    // Otherwise derive from the process role: only the stateless serve replica
    // skips the periodic driver (it offloads to the worker fleet). Single source
    // of truth so a single `OXY_ROLE=all` instance always drains its own queue.
    crate::server::role_manifest::role_runs_inprocess_workers(
        crate::server::role_manifest::current_process_role(),
    )
}

pub(super) fn inproc_global_worker_interval() -> std::time::Duration {
    let secs = std::env::var(INPROC_GLOBAL_WORKER_INTERVAL_ENV)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(DEFAULT_INPROC_GLOBAL_WORKER_INTERVAL_SECS);
    std::time::Duration::from_secs(secs)
}

/// Whether this process may run the **one-shot startup pass** of
/// [`spawn_recovery`] — the `recover_active_runs` sweep that resumes every
/// interrupted run, as opposed to the periodic stranded-run tick.
///
/// The two passes select differently, and only one of them is safe to run from
/// a process that did not host the coordinators:
///
/// - The periodic tick (`find_stuck_runs`) and the latency worker
///   (`find_pending_global_runs`) both carry a **queue predicate**. A run a
///   live coordinator is driving always has a `claimed` — or `queued` +
///   `scope_owned = true` — entry in `agentic_task_queue`, and both queries
///   exclude exactly that. They cannot poach a live run, from any process.
/// - The one-shot pass (`get_resumable_root_runs`) has **no queue predicate**.
///   Its only guard is the driver lease. Every driver takes it now (see below),
///   but a lease taken by a driver that has just died still reads live for up
///   to the TTL, and the pass also resumes `awaiting_input` and `shutdown`
///   roots, which no driver holds by design.
///
/// That is sound in `oxy serve` (role `ide` / `all`), the one process whose
/// own direct drives it is recovering. It is **not** the right pass for
/// `oxy worker`: a worker restart (rolling deploy, OOM, HPA scale-up) says
/// nothing about a run parked on human input or marked `shutdown` by the `ide`
/// — those belong to the process that will host their coordinator. Hence the
/// split.
///
/// **Every live driver now holds the driver lease** — recovery drives always
/// did, and direct drives (chat, builder, custom-app asks on a serve replica,
/// interactive automations and backfills) take it through
/// `agentic_runtime::lifecycle::driver_lease`. So the one-shot pass, which is
/// lease-gated, no longer selects a run a live peer is driving (a serve
/// replica's ask was the case that reached it). What the lease costs is that a
/// driver which died moments before this boot still looks live for up to
/// `DRIVER_LEASE_TTL_SECS`; hence the pass runs a second time after
/// [`LAPSED_LEASE_PASS_DELAY`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupPass {
    /// Run it — this process is the one whose death orphaned the runs
    /// (`oxy serve`, whose own direct-drives went down with it).
    Run,
    /// Skip it and go straight to the queue-gated loops (`oxy worker`, which
    /// hosts no coordinators of its own and cannot tell a live peer's run from
    /// an orphan). Costs at most `STRANDED_GRACE_SECS` + one tick of latency:
    /// `find_stuck_runs` selects `needs_resume` / `shutdown` too, so nothing
    /// is lost, only delayed.
    Skip,
}

/// When the one-shot startup pass runs a second time.
///
/// The previous process's drives held driver leases, heartbeated every third
/// of the TTL; a crash leaves them looking live for up to
/// `DRIVER_LEASE_TTL_SECS`, and the first pass skips them. By this delay every
/// dead driver's lease has lapsed, while a live peer's has been renewed, so the
/// second pass picks up exactly the runs the first one had to leave.
const LAPSED_LEASE_PASS_DELAY: std::time::Duration =
    std::time::Duration::from_secs(agentic_runtime::crud::DRIVER_LEASE_TTL_SECS as u64 + 15);

/// Spawn startup recovery in the background. Returns immediately.
///
/// The spawned task rebuilds per-workspace `PlatformContext` + `BuilderBridges`
/// and hands them to [`recover_active_runs`] so runs interrupted by a server
/// restart actually resume instead of sitting in `needs_resume` forever — see
/// [`StartupPass`] for which callers may do that.
///
/// When [`INPROC_GLOBAL_WORKER_ENV`] is set, the task does not stop after the
/// one-shot pass: it re-runs recovery on an interval (tied to the shutdown
/// token) so the queue has a durable consumer for `scope_owned = false` work.
/// Safe to run from several processes at once: every selection funnels through
/// `recover_single_run`'s `try_acquire_driver` CAS, so the loser of a race
/// skips.
///
/// NOTE: an earlier version of this comment called `get_resumable_root_runs`
/// driver-lease-gated while no direct drive took the lease, so a run a peer
/// process was direct-driving was selected anyway. Direct drives hold the
/// lease now (`agentic_runtime::lifecycle::driver_lease`), which is what makes
/// the second, delayed startup pass safe.
pub fn spawn_recovery(
    agentic_state: Arc<AgenticState>,
    mode: ServeMode,
    // The node's Layer-1 preagg cache, from the same `AppState` the HTTP
    // middleware reads. Threaded down to the two background context builders so
    // queued work — scheduled monitor scans, automations, agentic runs — resolves
    // rollups on the tier its request-driven twin does. Default (no cache) simply
    // compiles to warehouse SQL.
    preagg: PreaggCacheCtx,
    custom_tasks: CustomTasks,
    startup: StartupPass,
) {
    let db = agentic_state.db.clone();
    let runtime = agentic_state.runtime.clone();
    let schema_cache = Some(agentic_state.schema_cache.clone());
    let builder_test_runner: Option<Arc<dyn BuilderTestRunnerTrait>> =
        agentic_state.builder_test_runner.clone();
    let router = agentic_state.router.clone();
    let shutdown = agentic_state.shutdown_token.clone();
    // §12 FU4c: long-lived cache so repeated periodic ticks (and the
    // latency worker, below) reuse the per-workspace OxyProjectContext
    // instead of rebuilding WorkspaceManager every cycle.
    let ws_cache = super::workspace_cache::new_workspace_context_cache();

    // Which Global runs THIS process drives. Decided ONCE, here, for the life
    // of the process — role and env are both fixed at boot — and handed to
    // every selection this process makes: the latency worker's probe and gate
    // (every second), the periodic stranded tick (every `interval`) and the
    // one-shot startup pass. What the boot log announces is what all of them
    // apply; there is no second read for them to disagree through. The two
    // passes that reap — the tick and the startup pass — cannot be left out:
    // each re-queues a dead worker's claim and then selects the run it freed,
    // so a gate that skipped either would hand that worker's run to this node.
    let policy = super::drive_policy::drive_policy();

    // §12 FU4c latency worker — shared across local and cloud modes.
    // Drains `queued scope_owned=false` rows at sub-second latency
    // instead of waiting for the periodic loop's grace window.
    //
    // The cloud-mode branch (added when `agentic_runs.workspace_id`
    // landed) groups pending rows by workspace and routes each to the
    // right cached `PlatformContext` via `ws_cache`; no need for one
    // worker per workspace.
    if inproc_global_worker_enabled() {
        super::drive_policy::announce(policy);
        crate::server::compile_reconcile::announce();
        spawn_airhouse_tenant_sweeper(db.clone(), shutdown.clone());
        super::run_retention::spawn(db.clone(), shutdown.clone());
        spawn_latency_worker(
            db.clone(),
            runtime.clone(),
            schema_cache.clone(),
            builder_test_runner.clone(),
            router.clone(),
            mode,
            shutdown.clone(),
            ws_cache.clone(),
            preagg.clone(),
            custom_tasks.clone(),
            policy,
        );
    } else if matches!(mode, ServeMode::Cloud) {
        // Reached only when the global driver was explicitly disabled
        // (`OXY_INPROC_GLOBAL_WORKER=0`) on a non-serve node — the derived
        // default already turns it OFF for `serve` (which offloads to the worker
        // fleet) and ON for every other role. Loud signal because in cloud mode
        // with no node draining the queue, `TaskSpec::Compile` tasks (which the
        // compile boundary depends on) never run, so workspaces stay uncompiled
        // and unservable by the stateless fleet. Compile execution needs the
        // workspace working copy on disk, so the node running the driver must
        // have it. See internal-docs/compile-boundary.md.
        tracing::warn!(
            target: "recovery",
            "in-process global driver is explicitly OFF on a cloud node — queued \
             Global tasks (compiles in particular) will NOT be drained here. Ensure \
             a node with the workspace working copy runs the global driver (it is on \
             by default for every role except serve), or compiles never run. See \
             internal-docs/compile-boundary.md."
        );
    }

    tokio::spawn(async move {
        match startup {
            StartupPass::Run => {
                let recovered = run_recovery(
                    &db,
                    runtime.clone(),
                    schema_cache.clone(),
                    builder_test_runner.clone(),
                    router.clone(),
                    mode,
                    false, // one-shot startup recovery: every coordinator is dead
                    ws_cache.clone(),
                    preagg.clone(),
                    custom_tasks.clone(),
                    policy,
                )
                .await;
                if recovered > 0 {
                    tracing::info!(
                        target: "recovery",
                        recovered,
                        mode = mode.label(),
                        "startup recovery resumed interrupted runs"
                    );
                }
                // Once more after the lease TTL — see `LAPSED_LEASE_PASS_DELAY`.
                let (db, runtime, schema_cache, ws_cache, preagg, shutdown, custom_tasks) = (
                    db.clone(),
                    runtime.clone(),
                    schema_cache.clone(),
                    ws_cache.clone(),
                    preagg.clone(),
                    shutdown.clone(),
                    custom_tasks.clone(),
                );
                let (builder_test_runner, router) = (builder_test_runner.clone(), router.clone());
                tokio::spawn(async move {
                    tokio::select! {
                        _ = shutdown.cancelled() => return,
                        _ = tokio::time::sleep(LAPSED_LEASE_PASS_DELAY) => {}
                    }
                    let recovered = run_recovery(
                        &db,
                        runtime,
                        schema_cache,
                        builder_test_runner,
                        router,
                        mode,
                        false,
                        ws_cache,
                        preagg,
                        custom_tasks,
                        policy,
                    )
                    .await;
                    if recovered > 0 {
                        tracing::info!(
                            target: "recovery",
                            recovered,
                            mode = mode.label(),
                            "startup recovery resumed runs whose previous driver's lease lapsed"
                        );
                    }
                });
            }
            StartupPass::Skip => tracing::info!(
                target: "recovery",
                mode = mode.label(),
                "skipping the one-shot startup pass: this process hosts no \
                 coordinators, so a resumable-looking run may belong to a live \
                 peer. Stranded work is picked up by the queue-gated loops \
                 below instead"
            ),
        }

        if !inproc_global_worker_enabled() {
            return;
        }
        let interval = inproc_global_worker_interval();
        tracing::info!(
            target: "recovery",
            interval_secs = interval.as_secs(),
            mode = mode.label(),
            "in-process global driver loop enabled (OXY_INPROC_GLOBAL_WORKER)"
        );
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!(target: "recovery", "global driver loop: shutdown");
                    break;
                }
                _ = tokio::time::sleep(interval) => {
                    // Every eligible node drives this tick — no leader election
                    // needed. The work it fans out is already exactly-once across
                    // replicas: `tick_schedules` / `tick_monitor_schedules`
                    // CAS-advance `next_run_at` (only the replica whose UPDATE
                    // hits one row fires — see agentic-pipeline::scheduler), and
                    // stranded-run recovery is driver-lease-gated (F1). So N
                    // concurrent drivers self-dedupe, which is strictly better
                    // HA than a single leader: instant failover, no lease, no
                    // SPOF. The only cost is a little redundant polling, which is
                    // a cheap indexed query.
                    let n = run_recovery(
                        &db,
                        runtime.clone(),
                        schema_cache.clone(),
                        builder_test_runner.clone(),
                        router.clone(),
                        mode,
                        true, // periodic tick: stranded runs only (never poach a live interactive run)
                        ws_cache.clone(),
                        preagg.clone(),
                        custom_tasks.clone(),
                        policy,
                    )
                    .await;
                    if n > 0 {
                        tracing::info!(
                            target: "recovery",
                            picked_up = n,
                            "global driver loop: drove scope_owned=false runs"
                        );
                    }
                    // Held workspace-preview runs: finish those whose run is
                    // terminal, then start each workspace's next queued one.
                    // Row writes only — the runs themselves are queue work.
                    crate::server::previews::runs::sweep(&db).await;
                    // Token hygiene (spent OIDC jtis, long-expired `ci`
                    // tokens, expiry notices, unused tokens) is
                    // `oxy-api-identity`'s tick, run with the other surfaces'
                    // through `SurfaceSeams::global_ticks`.
                    // The weekly custom-app usage report: written once a week
                    // and emailed to the staff who want it. Claimed by a unique
                    // row per week and per address, throttled per process, so
                    // every driver running it writes and sends once between them.
                    crate::server::api::admin::usage_report::job::tick(&db).await;
                }
            }
        }
    });
}

/// Pure async recovery entry point — separated from [`spawn_recovery`] so
/// tests can drive it without a background task.
pub(super) async fn run_recovery(
    db: &DatabaseConnection,
    runtime: Arc<RuntimeState>,
    schema_cache: Option<
        Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, agentic_pipeline::AnalyticsSchemaCatalog>,
            >,
        >,
    >,
    builder_test_runner: Option<Arc<dyn BuilderTestRunnerTrait>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    mode: ServeMode,
    // `true` = periodic global-driver tick (drive stranded runs only);
    // `false` = one-shot startup recovery (drive all resumable runs).
    periodic: bool,
    ws_cache: Arc<super::workspace_cache::WorkspaceContextCache>,
    preagg: PreaggCacheCtx,
    custom_tasks: CustomTasks,
    // Applied by both passes: the periodic tick's `recover_stranded_runs`
    // gates everything it selects; the one-shot startup pass gates only the
    // roots another node's latency worker can see (see
    // `recover_active_runs`).
    policy: agentic_pipeline::recovery::DrivePolicy,
) -> usize {
    match mode {
        ServeMode::Local => {
            recover_local(
                db,
                runtime,
                schema_cache,
                builder_test_runner,
                router,
                periodic,
                ws_cache,
                preagg,
                custom_tasks,
                policy,
            )
            .await
        }
        // The periodic tick claims a bounded batch of workspaces instead of
        // visiting every one on every node — see `super::periodic`.
        ServeMode::Cloud if periodic => {
            super::periodic::tick(&super::periodic::TickDeps {
                db: db.clone(),
                runtime,
                schema_cache,
                builder_test_runner,
                router,
                ws_cache,
                preagg,
                custom_tasks,
                policy,
            })
            .await
        }
        ServeMode::Cloud => {
            recover_all_workspaces(
                db,
                runtime,
                schema_cache,
                builder_test_runner,
                router,
                ws_cache,
                preagg,
                custom_tasks,
                policy,
            )
            .await
        }
    }
}

async fn recover_local(
    db: &DatabaseConnection,
    runtime: Arc<RuntimeState>,
    schema_cache: Option<
        Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, agentic_pipeline::AnalyticsSchemaCatalog>,
            >,
        >,
    >,
    builder_test_runner: Option<Arc<dyn BuilderTestRunnerTrait>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    periodic: bool,
    ws_cache: Arc<super::workspace_cache::WorkspaceContextCache>,
    preagg: PreaggCacheCtx,
    custom_tasks: CustomTasks,
    policy: agentic_pipeline::recovery::DrivePolicy,
) -> usize {
    let cwd = match std::env::current_dir() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(target: "recovery", error = %e, "local recovery: no cwd, skipping");
            return 0;
        }
    };

    let Some(project_ctx) = ws_cache
        .get_or_build(LOCAL_WORKSPACE_ID, || async {
            build_local_project_ctx(&cwd, db, &preagg).await
        })
        .await
    else {
        return 0;
    };
    let platform: Arc<dyn PlatformContext> = project_ctx.clone();
    let platform_for_monitor: Arc<dyn PlatformContext> = project_ctx.clone();
    // Reused by the Phase 2 scheduler tick (airway seeds need a workspace
    // surface; OxyProjectContext is also a WorkflowWorkspaceContext).
    let workspace: Arc<dyn agentic_pipeline::WorkflowWorkspaceContext> = project_ctx.clone();
    let bridges: Option<BuilderBridges> = Some(build_builder_bridges(project_ctx));

    if periodic {
        // Local-mode workspace_id == LOCAL_WORKSPACE_ID (Uuid::nil()).
        // Stamped on every local run by the `start_*_run` paths; we
        // filter the selection by it so the SELECT can't drift across
        // workspaces if a cloud DB is ever pointed at a local server.
        let recovered = recover_stranded_runs(
            db.clone(),
            runtime,
            platform,
            CallerRunResolver::shared(db),
            bridges,
            schema_cache.clone(),
            builder_test_runner,
            router,
            Some(LOCAL_WORKSPACE_ID),
            Some(build_custom_task_registry(
                db,
                &preagg,
                &custom_tasks,
                schema_cache,
            )),
            policy,
        )
        .await;
        // Periodic tick drives the cron scheduler, scoped to this
        // workspace (§12 FU4b). Local mode = the single LOCAL_WORKSPACE_ID.
        let fired =
            agentic_pipeline::scheduler::tick_schedules(db, LOCAL_WORKSPACE_ID, workspace.as_ref())
                .await;
        if fired > 0 {
            tracing::info!(target: "scheduler", fired, workspace_id = %LOCAL_WORKSPACE_ID, "periodic tick fired schedules");
        }
        super::periodic::monitors::bootstrap_from_file(db, LOCAL_WORKSPACE_ID, &cwd).await;
        let monitor_fired = agentic_pipeline::scheduler::tick_monitor_schedules(
            db,
            LOCAL_WORKSPACE_ID,
            platform_for_monitor,
        )
        .await;
        if monitor_fired > 0 {
            tracing::info!(
                target: "metric_monitoring",
                fired = monitor_fired,
                workspace_id = %LOCAL_WORKSPACE_ID,
                "monitor tick enqueued scans"
            );
        }
        // Fire due per-workspace health rows. Cheap indexed SELECT of due rows
        // only; reconciliation of the rows themselves is NOT done here (see the
        // startup branch below).
        let health_fired = agentic_pipeline::scheduler::tick_health_schedules(db).await;
        if health_fired > 0 {
            tracing::info!(target: "health_eval", "health schedule fired");
        }
        let preagg_fired = agentic_pipeline::scheduler::tick_preagg_schedules(db).await;
        if preagg_fired > 0 {
            tracing::info!(target: "preagg", "preagg schedule fired");
        }
        // The surfaces' global ticks (`SurfaceSeams::global_ticks`; the experiment
        // tick, which advances due runs). Each is exactly-once across replicas by
        // its own CAS, like the schedule ticks above.
        super::seams::run_global_ticks(db, chrono::Utc::now()).await;
        recovered + fired
    } else {
        // Local mode: the single workspace has no `workspaces` row, so skip the
        // orphan prune (an empty table there is not evidence of orphans).
        super::periodic::reconcile::startup_housekeeping(db, false).await;
        recover_active_runs(
            db.clone(),
            runtime,
            platform,
            CallerRunResolver::shared(db),
            bridges,
            schema_cache.clone(),
            builder_test_runner,
            router,
            Some(LOCAL_WORKSPACE_ID),
            Some(build_custom_task_registry(
                db,
                &preagg,
                &custom_tasks,
                schema_cache,
            )),
            policy,
        )
        .await
    }
}

/// The one-shot startup pass over every workspace: resume each one's
/// interrupted runs. The periodic tick is [`super::periodic::tick`], which
/// claims a bounded batch instead of visiting every workspace.
async fn recover_all_workspaces(
    db: &DatabaseConnection,
    runtime: Arc<RuntimeState>,
    schema_cache: Option<
        Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, agentic_pipeline::AnalyticsSchemaCatalog>,
            >,
        >,
    >,
    builder_test_runner: Option<Arc<dyn BuilderTestRunnerTrait>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    ws_cache: Arc<super::workspace_cache::WorkspaceContextCache>,
    preagg: PreaggCacheCtx,
    custom_tasks: CustomTasks,
    policy: agentic_pipeline::recovery::DrivePolicy,
) -> usize {
    let workspaces = match entity::workspaces::Entity::find().all(db).await {
        Ok(ws) => ws,
        Err(e) => {
            tracing::error!(target: "recovery", error = %e, "cloud recovery: failed to list workspaces");
            return 0;
        }
    };

    if workspaces.is_empty() {
        tracing::info!(target: "recovery", "cloud recovery: no workspaces registered");
        return 0;
    }

    let mut total = 0usize;
    for ws in &workspaces {
        let Some(ref path) = ws.path else {
            continue;
        };

        let Some(project_ctx) = worker_ctx::worker_project_ctx(
            &ws_cache,
            ws.id,
            path,
            ws.current_revision_id,
            db,
            &preagg,
        )
        .await
        else {
            // build_cloud_project_ctx_inner already logged the failure cause.
            continue;
        };
        let platform: Arc<dyn PlatformContext> = project_ctx.clone();
        let bridges: Option<BuilderBridges> = Some(build_builder_bridges(project_ctx));

        let n = recover_active_runs(
            db.clone(),
            runtime.clone(),
            platform,
            CallerRunResolver::shared(db),
            bridges,
            schema_cache.clone(),
            builder_test_runner.clone(),
            router.clone(),
            Some(ws.id),
            Some(build_custom_task_registry(
                db,
                &preagg,
                &custom_tasks,
                schema_cache.clone(),
            )),
            policy,
        )
        .await;

        if n > 0 {
            tracing::info!(
                target: "recovery",
                workspace_id = %ws.id,
                recovered = n,
                "cloud recovery: resumed runs for workspace"
            );
        }
        total += n;
    }
    // Legacy-row and orphan cleanup, once per boot. The per-workspace cadence
    // backfill that used to run here is claim-gated in the periodic tick
    // (`super::periodic::reconcile`): a workspace is reconciled when its
    // revision changed or its hourly net comes round, not here on every node.
    super::periodic::reconcile::startup_housekeeping(db, true).await;
    total
}

/// Remove the Airhouse tenants that workspace deletes recorded, every
/// [`airhouse::workspace_delete::SWEEP_INTERVAL`]. On the nodes that drive the
/// queue (every role but serve); several sweep at once and each claims its own
/// rows. Not a queued run: the run driver retires every run whose workspace is
/// gone, which is all of these — see `airhouse::workspace_delete`.
fn spawn_airhouse_tenant_sweeper(
    db: sea_orm::DatabaseConnection,
    shutdown: tokio_util::sync::CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = tokio::time::sleep(airhouse::workspace_delete::SWEEP_INTERVAL) => {
                    airhouse::workspace_delete::sweep(&db).await;
                }
            }
        }
    });
}

// ── Per-workspace context builders (used by the cache, §12 FU4c) ───────────

/// §12 FU4c latency-worker spawn (local + cloud).
///
/// Polls `find_pending_global_runs` every ~1s and drives any
/// freshly-seeded `queued scope_owned = false` runs immediately, instead
/// of waiting for the periodic loop's grace window (≈ grace + interval).
/// Concurrency-safe vs. the periodic loop: both call `recover_single_run`
/// which CAS-acquires the driver lease, so the loser of any race skips.
///
/// **Local mode:** rebuild/reuse the single `LOCAL_WORKSPACE_ID` context
/// every tick and drive any pending row.
///
/// **Cloud mode:** discover which workspaces actually have pending rows
/// via `find_pending_global_runs(None)`, then drive each workspace's
/// rows with its own cached context. This is a single shared worker, not
/// one-per-workspace — the routing is per-row via `agentic_runs.workspace_id`.
#[allow(clippy::too_many_arguments)]
fn spawn_latency_worker(
    db: sea_orm::DatabaseConnection,
    runtime: Arc<RuntimeState>,
    schema_cache: Option<
        Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, agentic_pipeline::AnalyticsSchemaCatalog>,
            >,
        >,
    >,
    builder_test_runner: Option<Arc<dyn BuilderTestRunnerTrait>>,
    router: Arc<dyn agentic_runtime::router::TaskRouter>,
    mode: ServeMode,
    shutdown: tokio_util::sync::CancellationToken,
    ws_cache: Arc<super::workspace_cache::WorkspaceContextCache>,
    preagg: PreaggCacheCtx,
    custom_tasks: CustomTasks,
    // The process's one `drive_policy()` value, decided and announced by
    // `spawn_recovery`. Every tick is handed it; neither the probe nor the gate
    // re-reads the environment.
    policy: agentic_pipeline::recovery::DrivePolicy,
) {
    // Configurable for soak tuning; default 1s.
    let poll = match std::env::var("OXY_LATENCY_WORKER_INTERVAL_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
    {
        Some(ms) => std::time::Duration::from_millis(ms),
        None => std::time::Duration::from_millis(1000),
    };
    tracing::info!(
        target: "recovery",
        poll_ms = poll.as_millis() as u64,
        mode = mode.label(),
        "latency worker enabled (OXY_INPROC_GLOBAL_WORKER)"
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    tracing::info!(target: "recovery", "latency worker: shutdown");
                    break;
                }
                _ = tokio::time::sleep(poll) => {
                    // Race the whole tick against shutdown. A poll (or a run
                    // drive) already in flight when a co-located DB dies on
                    // shutdown would otherwise block the full pool
                    // `ACQUIRE_TIMEOUT` (~30s) before erroring, delaying the
                    // bounded shutdown-hook wait that follows serve. On
                    // shutdown we abandon the tick and break; any half-driven
                    // Global run is picked back up by recovery on restart (or
                    // by a peer), gated by the driver lease.
                    let tick = async {
                        match mode {
                            ServeMode::Local => {
                                tick_local(
                                    &db,
                                    &runtime,
                                    schema_cache.as_ref(),
                                    builder_test_runner.as_ref(),
                                    &router,
                                    &ws_cache,
                                    &preagg,
                                    &custom_tasks,
                                    policy,
                                )
                                .await
                            }
                            ServeMode::Cloud => {
                                tick_cloud(
                                    &db,
                                    &runtime,
                                    schema_cache.as_ref(),
                                    builder_test_runner.as_ref(),
                                    &router,
                                    &ws_cache,
                                    &preagg,
                                    &custom_tasks,
                                    policy,
                                )
                                .await
                            }
                        }
                    };
                    let n = tokio::select! {
                        _ = shutdown.cancelled() => {
                            tracing::info!(target: "recovery", "latency worker: shutdown during poll");
                            break;
                        }
                        n = tick => n,
                    };
                    if n > 0 {
                        tracing::debug!(
                            target: "recovery",
                            picked_up = n,
                            mode = mode.label(),
                            "latency worker: drove pending Global runs"
                        );
                    }
                }
            }
        }
    });
}

/// Local-mode tick: single cached context, no per-row routing needed.
///
/// One span per driver pass, which is what makes the `recovery`, `scheduler`,
/// `metric_monitoring`, `health_eval` and `preagg` lines emitted underneath it
/// correlate: they are the loudest untraced targets on `oxy-worker`, which
/// carried a trace id on **0** of 4,179 log lines
/// (`internal-docs/2026-09-09-hyperdx-observability-findings.md` §3). The span
/// sits on the outer pass rather than on each `tick_*` so a workspace fleet
/// does not multiply one heartbeat into hundreds of spans — one pass, one
/// trace, every line in it.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    target = "recovery",
    name = "driver_tick",
    skip_all,
    fields(mode = "local")
)]
async fn tick_local(
    db: &sea_orm::DatabaseConnection,
    runtime: &Arc<RuntimeState>,
    schema_cache: Option<
        &Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, agentic_pipeline::AnalyticsSchemaCatalog>,
            >,
        >,
    >,
    builder_test_runner: Option<&Arc<dyn BuilderTestRunnerTrait>>,
    router: &Arc<dyn agentic_runtime::router::TaskRouter>,
    ws_cache: &Arc<super::workspace_cache::WorkspaceContextCache>,
    preagg: &PreaggCacheCtx,
    custom_tasks: &[TaskSeam],
    policy: agentic_pipeline::recovery::DrivePolicy,
) -> usize {
    let Ok(cwd) = std::env::current_dir() else {
        return 0;
    };
    let Some(ctx) = ws_cache
        .get_or_build(LOCAL_WORKSPACE_ID, || async {
            build_local_project_ctx(&cwd, db, preagg).await
        })
        .await
    else {
        return 0;
    };
    drive_pending(
        db,
        runtime,
        ctx,
        schema_cache,
        builder_test_runner,
        router,
        Some(LOCAL_WORKSPACE_ID),
        preagg,
        custom_tasks,
        policy,
    )
    .await
}

/// Cloud-mode tick: discover which workspaces have pending rows, then
/// drive each one with its own cached context. We probe the unfiltered
/// SELECT once per tick to avoid scanning every workspace in the DB when
/// most have no work; the per-workspace re-select inside `drive_pending`
/// CAS-protects against double-drive across replicas.
///
/// Spanned for the same reason as [`tick_local`], and it matters more here:
/// cloud is the mode the fleet actually runs, so this is the root every
/// scheduler / recovery / monitor line on `oxy-worker` hangs off.
#[allow(clippy::too_many_arguments)]
#[tracing::instrument(
    target = "recovery",
    name = "driver_tick",
    skip_all,
    fields(mode = "cloud")
)]
async fn tick_cloud(
    db: &sea_orm::DatabaseConnection,
    runtime: &Arc<RuntimeState>,
    schema_cache: Option<
        &Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, agentic_pipeline::AnalyticsSchemaCatalog>,
            >,
        >,
    >,
    builder_test_runner: Option<&Arc<dyn BuilderTestRunnerTrait>>,
    router: &Arc<dyn agentic_runtime::router::TaskRouter>,
    ws_cache: &Arc<super::workspace_cache::WorkspaceContextCache>,
    preagg: &PreaggCacheCtx,
    custom_tasks: &[TaskSeam],
    policy: agentic_pipeline::recovery::DrivePolicy,
) -> usize {
    use std::collections::HashSet;
    let pending = match agentic_runtime::crud::find_pending_global_runs(db, None).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(target: "recovery", error = %e, "latency worker: cloud probe failed");
            return 0;
        }
    };
    if pending.is_empty() {
        return 0;
    }

    // A cheap probe-level skip, NOT the gate.
    //
    // This filters only which workspaces are visited: `drive_pending` re-selects
    // per workspace, and that second selection is what feeds the driver lease.
    // The real gate is the `policy` argument threaded into
    // `recover_pending_global_runs`. An earlier revision had only this
    // partition and believed it sufficient — it is not, because a workspace
    // with a compile AND any other pending Global run still reached the drive.
    //
    // Decline undrivable runs BEFORE the lease, not after the claim.
    //
    // A claim has no `task_kind` predicate, so any eligible driver wins
    // any row — but a node may be unable (compile without a working copy) or
    // unwanted (heavy work on the ide, see `drive_policy_for`) as its driver.
    // Failing after the claim makes success a coin flip on poll phase;
    // deferring after the claim is worse still, because only the lease-holder
    // may claim the row, so the deferring process re-selects its own work
    // every few seconds while its heartbeat excludes every other node from
    // `find_pending_global_runs` entirely — ending in a dead-lettered queue
    // row and a run stuck non-terminal.
    //
    // Skipping at selection leaves the run untouched, `driver_id IS NULL`, and
    // therefore selectable by the next eligible tick. That is the handoff the
    // other approaches only claimed.
    //
    // Same policy as the gate, and the same VALUE: `policy` is this tick's
    // argument, passed on to `drive_pending`, and split here by the same
    // `partition_drivable` the gate uses. A probe that skips MORE than the
    // gate silently hides drivable work, so neither consults the policy for
    // itself.
    // Kept whole for `retire_orphaned_runs` below. Retiring a run whose
    // workspace row is gone is a plain DB write that needs no workspace
    // context, so it is NOT a capability this node can lack — filtering it by
    // the driving exclusion would let an orphan of a declined kind sit
    // un-retired, which is the shape that pinned the pool on 2026-09-01.
    // Cheap: a clone of a handful of small structs, only on ticks that found
    // pending work at all.
    //
    // **Scope: the workspaces this node visits.** `workspaces` below is built
    // from the POST-partition list, so a deleted workspace whose only pending
    // runs are of a declined kind is never visited and `all_pending` is never
    // consulted for it. On a split fleet the other role covers that case — a
    // worker retires orphaned airway, an ide retires orphaned compile — so it
    // is not a stranding. Building `workspaces` from `all_pending` instead
    // would close it, at the cost of a per-tick workspace lookup (every 300 ms)
    // for workspaces this node has already decided it will not drive; that
    // trade is not obviously worth it, and this comment exists so the next
    // reader can make it deliberately rather than discover the gap.
    let all_pending = pending.clone();
    let (pending, declined) = agentic_pipeline::recovery::partition_drivable(pending, policy);
    if !declined.is_empty() {
        tracing::debug!(
            target: "recovery",
            count = declined.len(),
            role = crate::server::role_manifest::current_process_role().as_str(),
            policy = ?policy,
            "latency worker: leaving runs this node declines for one that takes them"
        );
    }
    if pending.is_empty() {
        return 0;
    }
    let workspaces: HashSet<uuid::Uuid> = pending.iter().map(|r| r.workspace_id).collect();

    let mut total = 0usize;
    for ws_id in workspaces {
        // Resolve workspace path from the workspaces table — cached
        // contexts are keyed by workspace_id, but the FIRST build per id
        // needs the path. Cheap one-row lookup; skipped on cache hits.
        //
        // The row also gives the promoted revision, which tags the cached
        // context: a promote since it was built rebuilds it (see
        // `worker_ctx::worker_project_ctx`).
        let (path, promoted) = match entity::workspaces::Entity::find_by_id(ws_id).one(db).await {
            Ok(found) => {
                let promoted = found.as_ref().and_then(|ws| ws.current_revision_id);
                match classify_workspace(found.map(|ws| ws.path)) {
                    WorkspaceOutcome::Drive(p) => (p, promoted),
                    WorkspaceOutcome::Retire(reason) => {
                        // `all_pending`, not `pending`. Once this workspace is known
                        // unrunnable, every run pointing at it is unrunnable
                        // regardless of whether THIS node would have driven it.
                        // Passing the filtered list leaves a declined kind — a
                        // compile on a worker, an airway run on a deferring ide —
                        // orphaned and re-selected forever.
                        //
                        // Merge note: this argument was written for the
                        // workspace-row-missing case and applies MORE broadly since
                        // #3062, which retires path-less workspaces through this same
                        // arm. Neither branch had both halves.
                        retire_orphaned_runs(db, ws_id, &all_pending, reason).await;
                        continue;
                    }
                }
            }
            // A lookup FAILURE is the one case that must still skip. It says
            // the database was briefly unreachable, not that the workspace is
            // gone — retiring on it would destroy live runs during a blip.
            Err(e) => {
                tracing::warn!(target: "recovery", error = %e, "latency worker: workspace lookup failed");
                continue;
            }
        };

        let Some(ctx) =
            worker_ctx::worker_project_ctx(ws_cache, ws_id, &path, promoted, db, preagg).await
        else {
            // Nothing is compiled for this workspace and this node cannot
            // compile a working copy, so no context to run its work under. A
            // commit compile is the one thing that needs none, and it is how a
            // workspace gets its first revision without the Factory.
            if has_commit_compile(&pending, ws_id)
                && let Some(ctx) = build_commit_compile_ctx(ws_id, &path, db, preagg).await
            {
                total += drive_pending(
                    db,
                    runtime,
                    ctx,
                    schema_cache,
                    builder_test_runner,
                    router,
                    Some(ws_id),
                    preagg,
                    custom_tasks,
                    COMMIT_COMPILES_ONLY,
                )
                .await;
            }
            continue;
        };
        total += drive_pending(
            db,
            runtime,
            ctx,
            schema_cache,
            builder_test_runner,
            router,
            Some(ws_id),
            preagg,
            custom_tasks,
            policy,
        )
        .await;
    }
    total
}

/// The only kind driven for a workspace with nothing compiled.
const COMMIT_COMPILES_ONLY: agentic_pipeline::recovery::DrivePolicy =
    agentic_pipeline::recovery::DrivePolicy::Exactly(&[
        agentic_runtime::coordinator::COMPILE_GIT_SOURCE_TYPE,
    ]);

/// Whether `workspace_id` has a pending commit compile this node may drive.
/// `pending` is already filtered by the node's own policy.
fn has_commit_compile(
    pending: &[agentic_runtime::crud::StuckRun],
    workspace_id: uuid::Uuid,
) -> bool {
    pending.iter().any(|r| {
        r.workspace_id == workspace_id
            && r.source_type.as_deref()
                == Some(agentic_runtime::coordinator::COMPILE_GIT_SOURCE_TYPE)
    })
}

/// What the latency worker should do with a workspace it has just looked up.
///
/// Extracted from [`tick_cloud`]'s match so the decision can be tested without
/// a database. The property worth protecting is not any single arm — it is that
/// **every** non-drivable outcome retires rather than skips, and that is exactly
/// what regressed: the deleted-row arm retired while the path-less arm next to
/// it went on skipping, which is the identical permanent hot loop one door over.
enum WorkspaceOutcome {
    /// The workspace resolves to a path; drive its pending runs.
    Drive(String),
    /// Unrunnable, and no future poll changes that. Retire with this reason.
    Retire(&'static str),
}

/// Classify a `workspaces` lookup that SUCCEEDED.
///
/// `None` — no such row. `Some(None)` — the row exists with no `path`.
/// `Some(Some(p))` — drivable. Deliberately takes the path rather than the
/// model: those two bits are the whole decision, and narrowing the input keeps
/// this callable from a test without constructing an entity.
///
/// A path-less workspace is not the transient state it looks like.
/// `register_project` writes `path` in the same `INSERT` that sets `status`, so
/// a path never "arrives later" — a row without one (local-mode sentinels,
/// demo/listing-only seeded rows) will not grow one, and a pending Global run
/// against it can never be driven.
fn classify_workspace(path: Option<Option<String>>) -> WorkspaceOutcome {
    match path {
        Some(Some(p)) => WorkspaceOutcome::Drive(p),
        Some(None) => WorkspaceOutcome::Retire("workspace has no path"),
        None => WorkspaceOutcome::Retire("workspace no longer exists"),
    }
}

/// Dead-letter every pending Global run on a workspace that cannot be resolved
/// to a path — whether the row is gone or is present without one. See
/// [`classify_workspace`], which decides between those and is the reason both
/// arrive here rather than only the first.
///
/// Such a run is unrunnable **by definition** — the tick needs the workspace
/// path to build a context and there is none, and no future poll will find one.
/// Skipping it therefore does not defer the work, it creates a permanent hot
/// loop: `find_pending_global_runs` re-selects the identical row on the very
/// next poll, on every node running a latency worker.
///
/// That loop took prod down on 2026-09-01. Two orphaned rows kept four
/// processes (`oxy-0` plus three workers) re-selecting at the 300 ms poll
/// interval around the clock; the connections that cost, on top of the other
/// resident loops, pinned `oxy_app` against Postgres' connection ceiling, and
/// the serve fleet — which shares that ceiling — started losing the race and
/// returning 500s after the full 30 s `ACQUIRE_TIMEOUT`. The user-visible
/// symptom was a homepage that intermittently hung for 30 s.
///
/// Terminal is the only correct outcome, so take it here.
async fn retire_orphaned_runs(
    db: &sea_orm::DatabaseConnection,
    ws_id: uuid::Uuid,
    pending: &[agentic_runtime::crud::StuckRun],
    reason: &'static str,
) {
    for run in pending.iter().filter(|r| r.workspace_id == ws_id) {
        // One transaction for both writes. Cancelling the queue rows is what
        // removes the run from `find_pending_global_runs`, so a terminal write
        // that then failed would strand the run non-terminal AND unselectable —
        // nothing left to retry it. `retire_run` rolls both back instead.
        match agentic_runtime::crud::retire_run(db, &run.run_id, reason).await {
            Ok(()) => tracing::warn!(
                target: "recovery",
                workspace_id = %ws_id,
                run_id = %run.run_id,
                reason,
                "latency worker: retired a pending run whose workspace cannot be resolved"
            ),
            Err(e) => tracing::warn!(
                target: "recovery",
                workspace_id = %ws_id,
                run_id = %run.run_id,
                error = %e,
                "latency worker: failed to retire an orphaned run; rolled back, \
                 retrying next tick"
            ),
        }
    }
}

/// Build the host-side registry of `TaskSpec::Custom` executors injected into
/// the global-run driver: the kinds `oxy-app` runs itself, then the ones the
/// extracted surfaces hand in (`SurfaceSeams::tasks`). See `internal-docs/agentic-runtime-integration.md`
/// ("One-shot queue work: `TaskSpec::Custom` + `CustomTaskRegistry`").
pub(super) fn build_custom_task_registry(
    db: &sea_orm::DatabaseConnection,
    preagg: &PreaggCacheCtx,
    tasks: &[TaskSeam],
    schema_cache: Option<crate::server::queued_analytics::SchemaCache>,
) -> Arc<agentic_runtime::worker::CustomTaskRegistry> {
    use crate::server::preagg_executor::PreaggTaskExecutor;
    let mut reg = agentic_runtime::worker::CustomTaskRegistry::new();
    // Insights delivery, queued after a monitor scan whose `.monitor.yml` has a
    // `notify:` block. Nothing can queue one before every pod parses that
    // block — an older pod refuses the file outright — so the kind and its
    // enqueue ship together; a pod without it fails the task (unknown kind) and
    // the events stay due for the next scan.
    {
        use crate::server::anomaly_notify::{ANOMALY_NOTIFY_KIND, executor::AnomalyNotifyExecutor};
        reg.register(
            ANOMALY_NOTIFY_KIND,
            Arc::new(AnomalyNotifyExecutor { db: db.clone() }),
        );
    }
    // A custom app's procedure run (`custom_app_procedure_run`) is
    // `oxy-api-custom-apps`' and arrives through `tasks` below; a process whose
    // seams lack it refuses to start (`require_task_kinds`).
    // A chat run (`/analytics`): every plain chat is queued by its start. It
    // runs as the requester the run records, never as this driver. (A custom
    // app's ask, its sibling, is `oxy-api-custom-apps`'s and arrives through
    // `tasks` below, with the same schema cache.)
    reg.register(
        agentic_pipeline::queued_run::CHAT_RUN_KIND,
        Arc::new(crate::server::queued_analytics::chat::ChatRunExecutor {
            db: db.clone(),
            schema_cache: schema_cache.clone(),
        }),
    );
    // Same shape as health eval: one executor instance, workspace context
    // rebuilt fresh per task from `workspace_id` in the payload. See
    // `preagg_executor`'s module doc for why this replaced a single
    // startup-bound worker.
    reg.register(
        "preagg_cycle",
        Arc::new(PreaggTaskExecutor { db: db.clone() }),
    );
    // Workspace previews' Airway change check. Reads the compile boundary and,
    // at most, a Reader on the workspace's Airhouse; a pod without it fails the
    // task (unknown kind) rather than running anything as production.
    reg.register(
        crate::server::previews::analyze::PREVIEW_ANALYZE_KIND,
        Arc::new(crate::server::previews::analyze::PreviewAnalyzeExecutor::airhouse(db.clone())),
    );
    // Workspace previews' TTL drop, queued by `previews::maintenance`. Drops
    // only schemas a registry row claimed by the run vouches for, through a
    // system Writer that sends fixed DDL; a pod without it fails the task.
    reg.register(
        crate::server::previews::drop::PREVIEW_SCHEMA_DROP_KIND,
        Arc::new(crate::server::previews::drop::PreviewSchemaDropExecutor::airhouse(db.clone())),
    );
    // Workspace previews' compare of a finished transform build with live,
    // queued by `previews::maintenance::enqueue_compares`. Reads only, through
    // the preview's Reader; its outcome is counts. A pod without it fails the
    // task.
    reg.register(
        crate::server::previews::compare::PREVIEW_COMPARE_KIND,
        Arc::new(crate::server::previews::compare::PreviewCompareExecutor::airhouse(db.clone())),
    );
    // Workspace previews' Airway sample, queued by `previews::runs::advance`.
    // Builds its own preview platform from the run row: preview-owned pipeline
    // name, lease and cursor; destination only in the preview's schemas on a
    // confined Writer. A pod without it fails the task (unknown kind).
    reg.register(
        crate::server::previews::sample::PREVIEW_AIRWAY_SAMPLE_KIND,
        Arc::new(
            crate::server::previews::sample::PreviewAirwaySampleExecutor::airhouse(db.clone()),
        ),
    );
    // The surfaces' kinds, handed in by the composition root
    // (`SurfaceSeams::tasks`): the custom-app kinds (a queued ask among them), the experiment and
    // simulation kinds (`oxy-api-experiment`: a simulation run, the price job,
    // read-back, the estimate, the interim reading), and the notification kinds
    // (`oxy-api-notifications`: a send's push, email and Slack delivery —
    // `ctx.notify`'s and every platform notice's, queued in the send's own
    // transaction — and the summary of the pushes quiet hours held), and the
    // per-workspace health eval (`oxy-api-workspace-health`). A kind absent here
    // fails its tasks as unknown, so `require_task_kinds` refuses to start a
    // process without the custom-app, notification and health ones, and
    // `oxy-server`'s own test holds the experiment ones to the composition.
    let deps = crate::server::router::TaskDeps {
        db,
        preagg,
        schema_cache: schema_cache.as_ref(),
    };
    for seam in tasks {
        reg.register(seam.kind, (seam.build)(&deps));
    }
    Arc::new(reg)
}

/// `recover_pending_global_runs` with the matching workspace filter.
#[allow(clippy::too_many_arguments)]
async fn drive_pending(
    db: &sea_orm::DatabaseConnection,
    runtime: &Arc<RuntimeState>,
    ctx: Arc<OxyProjectContext>,
    schema_cache: Option<
        &Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, agentic_pipeline::AnalyticsSchemaCatalog>,
            >,
        >,
    >,
    builder_test_runner: Option<&Arc<dyn BuilderTestRunnerTrait>>,
    router: &Arc<dyn agentic_runtime::router::TaskRouter>,
    workspace_id: Option<uuid::Uuid>,
    preagg: &PreaggCacheCtx,
    custom_tasks: &[TaskSeam],
    policy: agentic_pipeline::recovery::DrivePolicy,
) -> usize {
    let platform: Arc<dyn PlatformContext> = ctx.clone();
    let bridges: Option<BuilderBridges> = Some(build_builder_bridges(ctx));
    // The latency worker is where freshly-seeded Global runs (incl. per-workspace
    // `health_eval_workspace` Custom tasks) are drained, so inject the host's
    // Custom-kind executors here. Cheap to build per call (a few Arc clones).
    let custom_executors = Some(build_custom_task_registry(
        db,
        preagg,
        custom_tasks,
        schema_cache.cloned(),
    ));
    // The real gate: `policy` is checked inside `recover_pending_global_runs`,
    // immediately before `try_acquire_driver`, so a declined run keeps
    // `driver_id IS NULL` and stays selectable by a node that can take it. It
    // is the caller's value, not a fresh read — see `drive_policy::drive_policy_for`
    // for what is declined where, and why.
    agentic_pipeline::recovery::recover_pending_global_runs(
        db.clone(),
        runtime.clone(),
        platform,
        // Preview-owned roots get the preview platform, a custom-app ask its
        // caller's context; everything else `platform`.
        CallerRunResolver::shared(db),
        bridges,
        schema_cache.cloned(),
        builder_test_runner.cloned(),
        router.clone(),
        workspace_id,
        custom_executors,
        policy,
    )
    .await
}

async fn build_local_project_ctx(
    cwd: &std::path::Path,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<Arc<OxyProjectContext>> {
    let mut builder = match WorkspaceBuilder::new(LOCAL_WORKSPACE_ID)
        .with_working_copy(cwd, None, oxy::config::OnMissing::Empty)
        .await
    {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                cwd = %cwd.display(),
                error = %e,
                "local recovery: failed to resolve workspace path"
            );
            return None;
        }
    };
    // Attach the same DB-first (env-fallback) secrets manager the request
    // middleware builds — otherwise a resumed run can't resolve workspace
    // secrets stored in the DB (e.g. a ClickHouse `password_var`, a Toast
    // `TOAST_CLIENT_SECRET`) and only sees process env vars. This is the
    // difference between the resume path and a direct run.
    builder = with_db_secrets_manager(builder, LOCAL_WORKSPACE_ID);
    let wm = match builder.build().await {
        Ok(wm) => wm,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                cwd = %cwd.display(),
                error = %e,
                "local recovery: failed to build WorkspaceManager"
            );
            return None;
        }
    };
    // Wire the DB handle exactly like the HTTP workspace middleware does.
    // The in-process global driver executes `TaskSpec::Compile` through this
    // context, and `compile_dispatcher()` (like the anomaly tools) is `None`
    // unless `db` is set — without it every compile fails with
    // "compile_dispatcher() returned None". See OxyProjectContext::with_db.
    Some(Arc::new(
        OxyProjectContext::new(wm)
            .with_db(Arc::new(db.clone()))
            .with_preagg(preagg),
    ))
}

/// Attach the DB-backed (env-fallback) secrets manager to a workspace
/// builder, mirroring the request middleware. Best-effort: a failure logs
/// and leaves the builder's default manager, matching middleware behavior.
fn with_db_secrets_manager(
    builder: WorkspaceBuilder<WorkingCopy>,
    workspace_id: uuid::Uuid,
) -> WorkspaceBuilder<WorkingCopy> {
    match SecretsManager::from_database_with_env_fallback(SecretManagerService::new(workspace_id)) {
        Ok(secrets_manager) => builder.with_secrets_manager(secrets_manager),
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                %workspace_id,
                error = %e,
                "recovery: failed to create DB secrets manager; resumed run may not resolve DB secrets"
            );
            builder
        }
    }
}

/// Build a workspace-scoped `OxyProjectContext` for a background sweep given
/// only a `workspace_id` (resolving its path from the `workspaces` table).
/// Returns `None` when the workspace has no path (e.g. a local-mode sentinel) —
/// the caller degrades gracefully. Cloud path only; reuses
/// [`build_cloud_project_ctx_inner`]. Not cached here — callers that touch many
/// checks should build once per workspace and reuse.
pub async fn build_workspace_ctx(
    workspace_id: uuid::Uuid,
    db: &DatabaseConnection,
) -> Option<Arc<OxyProjectContext>> {
    let path = match entity::workspaces::Entity::find_by_id(workspace_id)
        .one(db)
        .await
    {
        Ok(Some(ws)) => ws.path?,
        Ok(None) => return None,
        Err(e) => {
            tracing::warn!(target: "recovery", %workspace_id, error = %e,
                "build_workspace_ctx: workspace lookup failed");
            return None;
        }
    };
    // No preagg context: this builder serves the workspace-health smoke probes
    // and the reconciliation check, both of which exist to test the WAREHOUSE.
    // Reconciliation compares an Oxy measure against a live external source, so
    // answering the Oxy side from a rollup would report drift that is really
    // rollup lag — the check would be measuring its own cache.
    let ctx =
        build_cloud_project_ctx_inner(workspace_id, &path, db, &PreaggCacheCtx::default()).await?;

    // Resolve the semantic scan root from the PROMOTED REVISION, and only from
    // it. Two failures, one line:
    //
    // 1. The eval runs as a queued task on the global-run fleet, which holds a
    //    workspace's ROW and not its DIRECTORY. Reading `working_copy().root()`
    //    there failed with `failed to load semantic model: failed to read
    //    <workspace path>: No such file or directory` — an alert about the
    //    node, worded as an alert about the workspace, on a workspace whose
    //    every other surface was querying that same model fine.
    // 2. On a node that DOES hold a working copy — a single instance, or the
    //    factory doubling as the global-run node — that copy holds whatever
    //    someone is editing in the IDE right now. Health judges the workspace,
    //    so it must judge what is promoted: an uncommitted `.view.yml` must
    //    never page the on-call about a revision that is fine.
    //
    // Hence `resolve_compiled_scan_source`, not `resolve_query_scan_source` —
    // the serving resolver falls back to the working copy by design, which is
    // right for every reader that is not this one.
    //
    // The scan is resolved ONCE and held for the life of the context: a
    // materialised scan is a tempdir, and every probe in the pass reads it.
    let ctx = ctx.requiring_compiled_semantic_scan();
    let ctx =
        match crate::server::api::semantic::resolve_compiled_scan_source(ctx.workspace_manager())
            .await
        {
            Ok(scan) => ctx.with_semantic_scan(scan),
            Err(unavailable) => {
                // Not fatal: the connection and app probes don't need a layer, and
                // the workspace is still evaluated on everything else. A compile is
                // enqueued, and the semantic checks report that the model is not
                // compiled rather than blaming the YAML — or, worse, judging
                // someone's draft.
                tracing::warn!(
                    target: "health_eval",
                    %workspace_id,
                    reason = %unavailable.message(),
                    "no compiled semantic model for the health eval; semantic checks will \
                     report unavailable until a compile lands"
                );
                ctx
            }
        };
    Some(Arc::new(ctx))
}

/// Why [`build_requester_base_ctx`] built nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequesterBaseError {
    /// Nothing is compiled and this node cannot compile: a compile was
    /// queued, and the same build succeeds once it lands.
    NotCompiled,
    /// The workspace manager would not build — a config that will not load
    /// here. Waiting does not change it.
    Build,
}

/// The workspace context a driver builds for a run it drives *as its
/// requester* (`queued_analytics::requester`): the tick's own construction —
/// the promoted revision, the DB-first secrets manager, the database handle —
/// before a subject and a role are put on it. No preagg context: the request
/// this stands in for read through the app's, which a driver does not hold;
/// a query then reads the warehouse instead of a rollup, never a wrong answer.
///
/// Says which of its two refusals it is, because a caller retries one and not
/// the other — as the build itself decided it, never from a second look at
/// state that can change in between.
pub(crate) async fn build_requester_base_ctx(
    workspace_id: uuid::Uuid,
    path: &str,
    db: &DatabaseConnection,
) -> Result<OxyProjectContext, RequesterBaseError> {
    build_cloud_project_ctx_result(workspace_id, path, db, &PreaggCacheCtx::default()).await
}

/// The revision a driver context built from `row` would pin now: the same
/// resolve [`build_cloud_project_ctx_inner`] makes, from a row the caller just
/// read. Lives here, beside the build, because this file is where the driver
/// resolves its `Origin` — the one place outside the request middleware allowed
/// to name `compiled_reader` (`tests/platform/compiled_reader_is_not_a_back_door.rs`).
pub(super) async fn driver_revision_for(row: &entity::workspaces::Model) -> Option<uuid::Uuid> {
    crate::server::api::compiled_reader::resolve_request_revision_for_workspace(row, None).await
}

/// The driver's workspace context, before the `Arc`, so a caller that has more
/// to attach (`build_workspace_ctx` resolves a semantic scan root, the driver's
/// cache a revision rebuilder — `worker_ctx::worker_project_ctx`) can do it
/// without unwrapping one.
pub(super) async fn build_cloud_project_ctx_inner(
    workspace_id: uuid::Uuid,
    path: &str,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<OxyProjectContext> {
    build_cloud_project_ctx_result(workspace_id, path, db, preagg)
        .await
        .ok()
}

/// [`build_cloud_project_ctx_inner`], saying why it built nothing.
async fn build_cloud_project_ctx_result(
    workspace_id: uuid::Uuid,
    path: &str,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Result<OxyProjectContext, RequesterBaseError> {
    // Resolve the config the same way the request middleware does: the promoted
    // compiled revision first, the working copy only on a miss.
    //
    // This is not just a cache — the two sources are not the same config. The
    // compile worker *injects* fields the on-disk `config.yml` never has, most
    // importantly the DuckDB `s3_mirror` block (`oxy-compile::duckdb_mirror`).
    // A context built from the FS therefore builds a `Local` DuckDB connector
    // pointed at a working copy that a stateless replica doesn't have, while
    // every request path — reading the compiled config — builds the S3-mirror
    // connector and succeeds. Background work (run recovery, reconciliation, the
    // health smoke probes) must see the same databases, with the same shape, as
    // the queries it is meant to be checking; otherwise the smoke test reports a
    // dead connection for a warehouse the product is happily querying.
    //
    // Branch is `None` (the promoted revision) to match the workspace root path
    // this context is built against — the same pairing `resolve_smoke_settings`
    // already uses.
    let compiled_config =
        crate::server::api::compiled_reader::resolve_request_revision(workspace_id, None).await;

    if compiled_config.is_none() && !crate::server::role_manifest::process_can_compile() {
        tracing::warn!(
            target: "recovery",
            %workspace_id,
            "no compiled config and no working copy on this node; enqueuing a compile"
        );
        crate::server::api::middlewares::workspace_context::enqueue_lazy_compile(db, workspace_id)
            .await;
        return Err(RequesterBaseError::NotCompiled);
    }
    assemble_cloud_project_ctx(workspace_id, path, compiled_config, db, preagg).await
}

/// The context a node that has nothing compiled for a workspace drives that
/// workspace's pending **commit compiles** under, and nothing else.
///
/// A commit compile reads no config from the context: it fetches its commit,
/// and the dispatcher resolves everything else from the database. What it does
/// need is a context that carries a database handle (the compile dispatcher is
/// `None` without one), so this is [`build_cloud_project_ctx_inner`] minus the
/// refusal to build one when nothing is compiled. It is never cached and never
/// given other work: the caller drives it under `DrivePolicy::Exactly`, because
/// an agent or a pipeline run against this empty config would fail where it
/// should wait for the compile.
async fn build_commit_compile_ctx(
    workspace_id: uuid::Uuid,
    path: &str,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Option<Arc<OxyProjectContext>> {
    assemble_cloud_project_ctx(workspace_id, path, None, db, preagg)
        .await
        .ok()
        .map(Arc::new)
}

async fn assemble_cloud_project_ctx(
    workspace_id: uuid::Uuid,
    path: &str,
    compiled_config: Option<uuid::Uuid>,
    db: &DatabaseConnection,
    preagg: &PreaggCacheCtx,
) -> Result<OxyProjectContext, RequesterBaseError> {
    let builder_init = WorkspaceBuilder::new(workspace_id)
        .with_working_copy(path, compiled_config, oxy::config::OnMissing::Empty)
        .await;
    let mut builder = match builder_init {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                %workspace_id,
                error = %e,
                "cloud recovery: failed to resolve workspace path, skipping"
            );
            return Err(RequesterBaseError::Build);
        }
    };
    // Same DB-first secrets manager as the request middleware, so a
    // resumed run resolves this workspace's DB-stored secrets (not just
    // process env). See `build_local_project_ctx`.
    builder = with_db_secrets_manager(builder, workspace_id);
    let wm = match builder.build().await {
        Ok(wm) => wm,
        Err(e) => {
            tracing::warn!(
                target: "recovery",
                %workspace_id,
                error = %e,
                "cloud recovery: failed to build WorkspaceManager, skipping"
            );
            return Err(RequesterBaseError::Build);
        }
    };
    // Wire the DB handle exactly like the HTTP workspace middleware does.
    // The in-process global driver executes `TaskSpec::Compile` through this
    // context, and `compile_dispatcher()` (like the anomaly tools) is `None`
    // unless `db` is set — without it every compile fails with
    // "compile_dispatcher() returned None". See OxyProjectContext::with_db.
    Ok(OxyProjectContext::new(wm)
        .with_db(Arc::new(db.clone()))
        .with_preagg(preagg))
}

pub(crate) mod worker_ctx;

#[cfg(test)]
mod agent_ask_tests;

#[cfg(test)]
mod procedure_run_tests;

#[cfg(test)]
mod seam_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The regression this file exists to prevent a second time.
    ///
    /// On 2026-09-01 the deleted-workspace arm was taught to retire while the
    /// path-less arm beside it kept skipping. Both are "the tick cannot get a
    /// path, and no later poll will produce one", so both are permanent hot
    /// loops — `find_pending_global_runs` re-selects an untouched row on the
    /// very next tick, forever. Asserting the arms individually would not have
    /// caught it; asserting that NO successful lookup can produce a skip does.
    #[test]
    fn every_unresolvable_workspace_retires_rather_than_skips() {
        for (label, input) in [("row deleted", None), ("row present, no path", Some(None))] {
            assert!(
                matches!(classify_workspace(input), WorkspaceOutcome::Retire(_)),
                "{label}: an unresolvable workspace must retire its runs, not skip                  them — skipping leaves the row selectable and re-polls it forever"
            );
        }
    }

    #[test]
    fn a_workspace_with_a_path_still_drives() {
        match classify_workspace(Some(Some("/workspace/oxy_data/ws".into()))) {
            WorkspaceOutcome::Drive(p) => assert_eq!(p, "/workspace/oxy_data/ws"),
            WorkspaceOutcome::Retire(r) => panic!("a drivable workspace was retired: {r}"),
        }
    }

    /// The two causes stay distinguishable in the retirement reason, which is
    /// what a future reader of `agentic_runs.error_message` has to work from.
    #[test]
    fn retirement_reasons_name_the_actual_cause() {
        let reason = |i| match classify_workspace(i) {
            WorkspaceOutcome::Retire(r) => r,
            WorkspaceOutcome::Drive(_) => panic!("expected a retirement"),
        };
        assert_eq!(reason(None), "workspace no longer exists");
        assert_eq!(reason(Some(None)), "workspace has no path");
    }
}

#[cfg(test)]
mod health_scan_wiring_tests {
    /// Source up to this module's own test attribute — the same shape guard
    /// `api::metric_tree::handlers_never_scan_the_working_copy` uses.
    fn src() -> &'static str {
        include_str!("recovery.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("source splits on its own test-module attribute")
    }

    /// `build_workspace_ctx` is the ONLY context builder the workspace-health
    /// eval uses. That eval judges a workspace, so it must read the promoted
    /// revision and nothing else: the fleet node it runs on may not hold the
    /// workspace directory at all, and a node that does holds whatever is being
    /// edited in the IDE.
    ///
    /// Guarded as source shape because the regression is an ABSENCE: dropping
    /// either call compiles perfectly and silently restores the working-copy
    /// fallback, which is only wrong in cloud. Poke House Staging paged for 58
    /// days on exactly that. `resolve_query_scan_source` is named here as the
    /// wrong door — it falls back to the working copy by design.
    #[test]
    fn the_health_context_reads_the_promoted_revision_and_nothing_else() {
        let s = src();
        let body = s
            .split("pub async fn build_workspace_ctx")
            .nth(1)
            .expect("build_workspace_ctx is defined here");
        assert!(
            body.contains("resolve_compiled_scan_source") && body.contains("with_semantic_scan"),
            "build_workspace_ctx must hand its context a compile-boundary scan root; \
             without one the health eval parses the semantic model out of a working \
             copy the fleet does not have"
        );
        assert!(
            body.contains("requiring_compiled_semantic_scan"),
            "and it must REFUSE the working-copy fallback: on a node that does hold the \
             files, falling back judges someone's uncommitted IDE edit and pages the \
             on-call about a promoted revision that is fine"
        );
    }
}

#[cfg(test)]
mod experiment_tick_wiring_tests {
    fn src() -> &'static str {
        include_str!("recovery.rs")
            .split("\n#[cfg(test)]")
            .next()
            .expect("source splits on its own test-module attribute")
    }

    fn between<'a>(s: &'a str, start: &str, end: &str) -> &'a str {
        s.split(start)
            .nth(1)
            .and_then(|rest| rest.split(end).next())
            .unwrap_or_else(|| panic!("`{start}` … `{end}` not found"))
    }

    /// The regression is an ABSENCE: drop the call and everything compiles,
    /// and no experiment ever advances on that path.
    #[test]
    fn both_global_driver_passes_tick_experiments() {
        let s = src();
        let local = between(
            s,
            "async fn recover_local(",
            "async fn recover_all_workspaces(",
        );
        let cloud = include_str!("periodic/mod.rs");
        for (pass, body) in [("local", local), ("cloud", cloud)] {
            assert!(
                body.contains("run_global_ticks("),
                "the {pass} global-driver pass must run the surfaces' global ticks \
                 (the experiment tick among them)"
            );
        }
    }
}
