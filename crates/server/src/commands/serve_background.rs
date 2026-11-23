//! `oxy serve`'s boot jobs: the background work a serve process runs beside its
//! routes, started from the `AgenticState` the router builders hand back.
//!
//! This was the boot section of `oxy-app`'s `api_router` until decompose BL-W
//! (2026-11-23); the router builders now only build, and `oxy-app`'s
//! `boot_guard_tests` fails if one starts any of this again. The body and its
//! order are as they were. What it starts, in order:
//!
//! 1. the surfaces' global ticks (installed, whatever the role);
//! 2. the startup pass, the global driver loop and the latency worker
//!    (`spawn_recovery`), unless `--no-workers`;
//! 3. the graceful-shutdown hook;
//! 4. compile-boundary maintenance, the audit prune and the token-usage flush;
//! 5. the surfaces' background jobs, then the fleet jobs (behind
//!    `OXY_INPROC_GLOBAL_WORKER`);
//! 6. the five camera-fleet loops.
//!
//! The internal router's shutdown hook is spawned by
//! [`internal_api_router_with_hook`], after this, as it was.
//!
//! `new_agentic_state` — the task-router listener, the reaper and the boot
//! cleanup — still runs inside the builders: the routes need the state, and it
//! leaves with the router-assembly lift. The drivers this calls are
//! `crate::drivers`, up from `oxy-app` since decompose BL1.

use std::sync::Arc;

use crate::router::{
    ApiRouterArgs, FleetJobSeam, GlobalTickSeam, SurfaceSeams, TaskSeam, api_router_with_state,
    internal_api_router,
};
use agentic_http::AgenticState;
use axum::Router;
use oxy_app::host::worker::PreaggCacheCtx;
use oxy_app_core::seams::BackgroundSeam;
use oxy_app_core::serve_mode::ServeMode;
use oxy_shared::errors::OxyError;
use tokio_util::sync::CancellationToken;

use crate::drivers::fleet_jobs::start_fleet_jobs;
use crate::drivers::recovery::{
    StartupPass, inproc_global_worker_enabled, register_shutdown_hook, spawn_recovery,
    spawn_shutdown_hook,
};

/// The seams' jobs, taken off them before the router consumes the rest.
struct SeamJobs {
    tasks: Vec<TaskSeam>,
    background: Vec<BackgroundSeam>,
    fleet_jobs: Vec<FleetJobSeam>,
    global_ticks: Vec<GlobalTickSeam>,
}

impl SeamJobs {
    fn take(seams: &mut SurfaceSeams) -> Self {
        Self {
            tasks: std::mem::take(&mut seams.tasks),
            background: std::mem::take(&mut seams.background),
            fleet_jobs: std::mem::take(&mut seams.fleet_jobs),
            global_ticks: std::mem::take(&mut seams.global_ticks),
        }
    }
}

/// Build the main API router, then start the boot jobs from its state:
/// `api_router` as it was before BL-W, boot included.
pub(crate) async fn api_router_and_boot(
    mut args: ApiRouterArgs,
) -> Result<(Router, Router, PreaggCacheCtx), OxyError> {
    let jobs = SeamJobs::take(&mut args.seams);
    let (mode, disable_inprocess_workers) = (args.mode, args.disable_inprocess_workers);
    let (router, external_router, preagg, agentic_state) = api_router_with_state(args)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("Failed to create API router: {}", e)))?;
    let boot = ServeBoot {
        agentic_state,
        mode,
        preagg: preagg.clone(),
        disable_inprocess_workers,
    };
    boot.start(jobs);
    Ok((router, external_router, preagg))
}

/// Build the internal API router, then spawn its shutdown hook, which
/// `internal_api_router` spawned itself before BL-W.
pub(crate) async fn internal_api_router_with_hook(
    enterprise: bool,
    observability: Option<Arc<dyn oxy_observability::ObservabilityStore>>,
    shutdown_token: CancellationToken,
    seams: SurfaceSeams,
) -> Result<Router, OxyError> {
    let (router, agentic_state) =
        internal_api_router(enterprise, observability, shutdown_token, seams).await?;
    spawn_shutdown_hook(agentic_state);
    Ok(router)
}

/// What the boot jobs start from.
struct ServeBoot {
    agentic_state: Arc<AgenticState>,
    mode: ServeMode,
    /// The same Layer-1 preagg cache the request path uses.
    preagg: PreaggCacheCtx,
    disable_inprocess_workers: bool,
}

impl ServeBoot {
    /// Every boot job, in the order `api_router` started them.
    fn start(self, jobs: SeamJobs) {
        self.start_drivers(jobs.global_ticks, jobs.tasks);
        self.start_maintenance();
        self.start_surface_jobs(&jobs.background, &jobs.fleet_jobs);
        self.start_camera_loops();
    }

    fn start_drivers(&self, global_ticks: Vec<GlobalTickSeam>, tasks: Vec<TaskSeam>) {
        // The surfaces' singleton ticks (`SurfaceSeams::global_ticks`), read by the
        // global driver each pass. Installed whether or not this process runs the
        // driver: the list is the composition's, not the role's.
        oxy_app_core::seams::install_global_ticks(&global_ticks);

        if self.disable_inprocess_workers {
            tracing::info!(
                "serve: --no-workers / OXY_DISABLE_INPROCESS_WORKERS active; skipping \
                 startup recovery and global driver loop. A separate `oxy worker` \
                 fleet must drive the agentic_task_queue."
            );
        } else {
            // `StartupPass::Run`: this process hosts the request-time coordinators
            // (and their direct-drives), so anything left resumable in the DB at
            // boot was orphaned by the process this one replaces. `oxy worker`
            // cannot make that claim and passes `Skip` — see [`StartupPass`].
            //
            // The preagg cache is the same `Arc` the request path uses, so queued
            // work — scheduled monitor scans, automations, agentic runs — resolves
            // rollups on the tier its request-driven twin does.
            spawn_recovery(
                self.agentic_state.clone(),
                self.mode,
                self.preagg.clone(),
                std::sync::Arc::from(tasks),
                StartupPass::Run,
            );
        }
        spawn_shutdown_hook(self.agentic_state.clone());
        // The custom-app automation runs' TTL sweep is `oxy-api-custom-apps`' and
        // starts with the surface jobs below (`custom_app_automation_run_sweep`).
    }

    fn start_maintenance(&self) {
        // Compile-boundary maintenance: reap stuck `compiling` revisions (crashed
        // compiles) + prune old non-current revisions so the `*_definitions`
        // tables stay bounded. Pure DB sweeps — runs regardless of --no-workers,
        // idempotent across replicas.
        oxy_compile_runtime::maintenance::spawn_compile_maintenance(
            oxy_compile_runtime::maintenance::CompileMaintenanceConfig::from_env(),
        );

        // Workspace previews' TTL sweep is `oxy-api-previews`'s
        // (`preview_maintenance`), started with the surfaces' background jobs.

        // Audit retention: the log is write-heavy, so keep a rolling 30-day window.
        // Idempotent daily sweep; verification anchors on the oldest retained event so a
        // prune doesn't report a false chain break.
        oxy_app_core::audit::spawn_audit_prune_loop();

        // Per-token usage: counted in memory by `token_usage_middleware`, written
        // here once a minute, and once more when the shutdown token is cancelled —
        // registered as a shutdown hook so the last partial minute is not lost.
        register_shutdown_hook(crate::drivers::token_usage_flush::spawn_token_usage_flush(
            self.agentic_state.shutdown_token.clone(),
        ));
    }

    fn start_surface_jobs(&self, background: &[BackgroundSeam], fleet_jobs: &[FleetJobSeam]) {
        // The jobs the composition root hands in for custom apps (`SurfaceSeams::
        // background`): the usage drain that batch-inserts page-view and beacon
        // rows, sandbox expiry, usage retention. A job that flushes its memory on
        // shutdown returns its handle; it is registered as a shutdown hook like the
        // token-usage flush above. Each is in-process, not a `TaskSpec`: it acts on
        // this process's memory or is a cheap idempotent sweep that every replica
        // runs.
        let deps = oxy_app_core::seams::BackgroundDeps {
            db: &self.agentic_state.db,
            shutdown: &self.agentic_state.shutdown_token,
        };
        for job in background {
            if let Some(handle) = (job.start)(&deps) {
                register_shutdown_hook(handle);
            }
        }

        // Fleet jobs (the storage sweeper, the usage-backfill sweep, the bucket
        // lifecycle check, the hourly audit chain-head anchor): started here only
        // where this serve runs singleton work (`all` / `ide`). On a split fleet
        // that is false and `oxy worker` starts them; either way each claims its
        // interval in `fleet_job_claims`, so starting them on both is safe.
        start_fleet_jobs(
            fleet_jobs,
            &oxy_app_core::seams::FleetJobDeps {
                db: &self.agentic_state.db,
                shutdown: &self.agentic_state.shutdown_token,
            },
            inproc_global_worker_enabled(),
        );
    }

    fn start_camera_loops(&self) {
        let db = &self.agentic_state.db;
        let shutdown = &self.agentic_state.shutdown_token;
        // Camera fleet stale-checker: flips edge_boxes.status to 'offline'
        // when last_seen_at goes silent past STALE_THRESHOLD. Bound to the
        // same shutdown token as the rest of the agentic state so it exits
        // cleanly on SIGTERM. The inverse transition (offline → active) is
        // handled by the auth middleware on the next /control/* call from
        // the box, so this loop is one-way only.
        oxy_cameras::service::stale::spawn(db.clone(), shutdown.clone());

        // Camera fleet log retention sweep (IoT Phase 6a). DELETEs old
        // rows from `oxy_cam_device_logs` per the configured policy
        // (default: info/debug 7d, warn+ 30d). Honors
        // OXY_CAMERA_LOG_SWEEP_INTERVAL_HOURS=0 to disable it; this loop
        // is the only sweep (the `oxy cameras sweep-logs` CLI was deleted).
        oxy_cameras::service::log_retention::spawn(db.clone(), shutdown.clone());

        // OTA rollout supervisor (P1 OTA #3). Advances `camera_rollout_plans`
        // rows through pending → canary → promoting → complete, auto-aborts
        // when canary failure rate exceeds threshold. Same shutdown token
        // as the other camera-domain loops.
        oxy_cameras::service::rollouts::spawn(db.clone(), shutdown.clone());

        // Camera health → Slack alerter (P1 #2). Polls camera_health
        // summary every 60s per workspace, diffs against the previous
        // tick, emits a Slack message on transitions to/from `ok` with
        // a 30-min per-camera cooldown. No-op when no workspace has a
        // Slack installation.
        oxy_cameras::service::alerts::spawn(db.clone(), shutdown.clone());

        // Camera ingest write-coalescing flusher: buffers high-frequency
        // events/health and flushes one batched INSERT per (tenant, stream) per
        // window, so airhouse sees ~one DuckLake commit/window instead of one per
        // edge POST. No-op (synchronous writes) under
        // OXY_CAMERAS_INGEST_BUFFER_DISABLED. Same shutdown token as the other
        // camera loops (final-drains on SIGTERM).
        oxy_cameras::service::ingest_buffer::spawn(shutdown.clone());

        // The world-model live feed's tailer and reaper, and the bridge that puts
        // camera domain events on it, are `oxy-api-world-model`'s: started above
        // with the other surface jobs (`SurfaceSeams::background`).
    }
}

#[cfg(test)]
#[path = "serve_background_tests.rs"]
mod tests;
