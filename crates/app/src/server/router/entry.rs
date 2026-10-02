//! Public entry points that assemble the full router.
//!
//! [`api_router`] is the user-facing API (cloud or local, driven by
//! [`ServeMode`]). [`internal_api_router`] is the internal port — always
//! cloud-shape, protected by an API-key-only middleware.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware;
use sentry::integrations::tower::NewSentryLayer;
use tower::ServiceBuilder;
use tower_http::timeout::TimeoutLayer;

use agentic_http::{AgenticState, cleanup_stale_runs};
use oxy_auth::middleware::internal_auth_middleware;
use oxy_shared::errors::OxyError;
use tokio_util::sync::CancellationToken;

use crate::agentic_wiring::builder_bridges::OxyBuilderAppRunner;
use crate::api::middlewares::timeout::timeout_middleware;
use crate::server::api::middlewares::workspace_context::PreaggCacheCtx;
use crate::server::builder_test_runner::OxyTestRunner;
use oxy_app_core::serve_mode::ServeMode;

use super::protected::{
    apply_local_middleware, apply_middleware, build_external_api_router,
    build_local_protected_routes, build_protected_routes,
};
use super::public::build_public_routes;
use super::recovery::{StartupPass, spawn_recovery, spawn_shutdown_hook};
use super::seams::{SurfaceSeam, SurfaceSeams};
use super::{AppState, build_cors_layer};

/// Builds the main API router (mounted under `/api`) and, alongside it, the
/// external API router (`/external/api`). The external one is returned
/// separately so the caller can mount it OUTSIDE the global CORS layer; see
/// [`build_external_api_router`].
pub async fn api_router(
    mode: ServeMode,
    enterprise: bool,
    observability: Option<std::sync::Arc<dyn oxy_observability::ObservabilityStore>>,
    startup_cwd: std::path::PathBuf,
    shutdown_token: CancellationToken,
    disable_inprocess_workers: bool,
    // Surface routes composed by the caller (the top `oxy-server` crate), by
    // which side of the auth stack each lands on — see [`SurfaceSeams`]. The
    // api seam joins the protected tree BEFORE `apply_middleware` (cloud mode
    // only: local mode never mounts the org/global tree and drops it); the
    // workspace seam goes INSIDE the `/{workspace_id}` nest; the public seam
    // merges beside `build_public_routes` in BOTH modes, with no auth layer.
    seams: SurfaceSeams,
) -> Result<(Router, Router, PreaggCacheCtx), OxyError> {
    let SurfaceSeams {
        api:
            SurfaceSeam {
                routes: extra_api_routes,
                decls: extra_api_decls,
            },
        workspace:
            SurfaceSeam {
                routes: extra_workspace_routes,
                decls: extra_workspace_decls,
            },
        public:
            SurfaceSeam {
                routes: extra_public_routes,
                decls: extra_public_decls,
            },
    } = seams;
    let agentic_state = new_agentic_state(shutdown_token, true).await?;

    // Layer-1 (per-query) preagg refresh-key cache, shared with every request
    // via `AppState` → `PreaggCacheCtx`. Node-local and workspace-agnostic (a
    // rollup hash is already unique per workspace+view+rollup, so one process
    // -wide map is fine) — always constructed for the main API router.
    //
    // The BACKGROUND rebuild cycle (Layer 2) is no longer spawned here: it used
    // to be one in-process loop bound to `startup_cwd`, which only ever built
    // pre-aggregations for whatever workspace happened to be checked out at the
    // server's own working directory — never a real tenant workspace in cloud
    // mode. It is now a per-workspace `preagg_cycle` schedule row (see
    // `agentic_pipeline::scheduler::{reconcile_preagg_schedule,
    // tick_preagg_schedules}`), reconciled from each workspace's own compiled
    // config and drained by the worker fleet via `PreaggTaskExecutor` — the same
    // shape as `health_eval_workspace`. `preagg_renewal_threshold_secs` was the
    // single global default this startup-bound worker exported; there is no
    // longer a meaningful single value to publish here, and `None` is now the
    // right answer rather than a silent fallback — `workspace_context` resolves
    // the threshold per request from THAT workspace's own
    // `pre_aggregations.refresh_worker.renewal_threshold`, which is the same
    // key the rebuild cycle reads. Publishing a process-wide number here would
    // outrank it.
    let (preagg_cache, preagg_renewal_threshold_secs): (
        Option<
            std::sync::Arc<std::sync::RwLock<agentic_semantic::refresh_key_cache::RefreshKeyCache>>,
        >,
        Option<u64>,
    ) = if !startup_cwd.as_os_str().is_empty() {
        (
            Some(std::sync::Arc::new(std::sync::RwLock::new(
                agentic_semantic::refresh_key_cache::RefreshKeyCache::new(),
            ))),
            None,
        )
    } else {
        (None, None)
    };

    if disable_inprocess_workers {
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
            agentic_state.clone(),
            mode,
            PreaggCacheCtx {
                cache: preagg_cache.clone(),
                renewal_threshold_secs: preagg_renewal_threshold_secs,
            },
            StartupPass::Run,
        );
    }
    spawn_shutdown_hook(agentic_state.clone());
    // Periodic maintenance for custom-app procedure runs: TTL
    // sweep + cross-instance cancel reconciliation + stuck-row
    // recovery. Cheap (delete-many + two update-many every 10 min)
    // and keeps the table bounded under load.
    crate::server::api::projects::automation_run::spawn_periodic_sweep(
        agentic_state.db.clone(),
        agentic_state.shutdown_token.clone(),
    );
    // One-shot: verify the custom-app asset bucket enforces the TTL classes this
    // binary stamps on objects. Singleton-gated because the answer is global, not
    // per-tenant, and identical from every replica.
    // Read-only: Terraform owns the asset bucket's lifecycle rules; this checks
    // they still match the TTL classes this build stamps and logs loudly if not.
    crate::server::api::custom_apps_storage::retention::spawn_lifecycle_verify(
        super::recovery::inproc_global_worker_enabled(),
    );
    // Periodic: re-measure each app's asset silo into `app_storage_usage` so
    // quotas, the admin fleet view, and GB-month billing have a number to read.
    // Singleton-gated — every replica sweeping would multiply LIST cost by the
    // replica count for identical results.
    crate::server::api::custom_apps_storage::sweeper::spawn_periodic_sweep(
        agentic_state.db.clone(),
        agentic_state.shutdown_token.clone(),
        super::recovery::inproc_global_worker_enabled(),
    );

    // Compile-boundary maintenance: reap stuck `compiling` revisions (crashed
    // compiles) + prune old non-current revisions so the `*_definitions`
    // tables stay bounded. Pure DB sweeps — runs regardless of --no-workers,
    // idempotent across replicas.
    crate::server::compile_maintenance::spawn_compile_maintenance(
        crate::server::compile_maintenance::CompileMaintenanceConfig::from_env(),
    );

    // Workspace previews' TTL sweep: claim the registry rows of expired
    // preview schemas and queue their `preview_schema_drop` tasks (the worker
    // fleet drops). Only writes rows; runs regardless of --no-workers,
    // idempotent across replicas.
    crate::server::previews::maintenance::spawn(
        crate::server::previews::maintenance::MaintenanceConfig::from_env(),
    );

    // Keep `origin/*` tracking refs warm so every surface that reports remote
    // state (compile freshness badge, ahead/behind counts) answers from a
    // recent fetch instead of whenever the user last happened to fetch by hand.
    // Fetch-only: never touches HEAD, the index, or the working tree.
    crate::server::git_fetch_maintenance::spawn_git_fetch_maintenance(
        crate::server::git_fetch_maintenance::GitFetchMaintenanceConfig::from_env(),
    );

    // Audit retention: the log is write-heavy, so keep a rolling 30-day window.
    // Idempotent daily sweep; verification anchors on the oldest retained event so a
    // prune doesn't report a false chain break.
    oxy_app_core::audit::spawn_audit_prune_loop();

    // Audit tamper evidence: every hour, each org's chain head goes to S3 under
    // Object Lock (compliance mode), so a verifier can check the database
    // against something the database cannot rewrite. Singleton-gated; a no-op
    // without `OXY_AUDIT_ANCHOR_S3_BUCKET`.
    crate::server::audit_anchor::spawn_audit_anchor_loop(
        agentic_state.db.clone(),
        agentic_state.shutdown_token.clone(),
        super::recovery::inproc_global_worker_enabled(),
    );

    // Camera fleet stale-checker: flips edge_boxes.status to 'offline'
    // when last_seen_at goes silent past STALE_THRESHOLD. Bound to the
    // same shutdown token as the rest of the agentic state so it exits
    // cleanly on SIGTERM. The inverse transition (offline → active) is
    // handled by the auth middleware on the next /control/* call from
    // the box, so this loop is one-way only.
    oxy_cameras::service::stale::spawn(
        agentic_state.db.clone(),
        agentic_state.shutdown_token.clone(),
    );

    // Camera fleet log retention sweep (IoT Phase 6a). DELETEs old
    // rows from `oxy_cam_device_logs` per the configured policy
    // (default: info/debug 7d, warn+ 30d). Honors
    // OXY_CAMERA_LOG_SWEEP_INTERVAL_HOURS=0 to disable when
    // operators prefer the `oxy cameras sweep-logs` CLI on cron.
    oxy_cameras::service::log_retention::spawn(
        agentic_state.db.clone(),
        agentic_state.shutdown_token.clone(),
    );

    // OTA rollout supervisor (P1 OTA #3). Advances `camera_rollout_plans`
    // rows through pending → canary → promoting → complete, auto-aborts
    // when canary failure rate exceeds threshold. Same shutdown token
    // as the other camera-domain loops.
    oxy_cameras::service::rollouts::spawn(
        agentic_state.db.clone(),
        agentic_state.shutdown_token.clone(),
    );

    // Camera health → Slack alerter (P1 #2). Polls camera_health
    // summary every 60s per workspace, diffs against the previous
    // tick, emits a Slack message on transitions to/from `ok` with
    // a 30-min per-camera cooldown. No-op when no workspace has a
    // Slack installation.
    oxy_cameras::service::alerts::spawn(
        agentic_state.db.clone(),
        agentic_state.shutdown_token.clone(),
    );

    // Worktree reaper (plan-2 lifecycle broker, step 1): on the `ide`
    // singleton, reclaim git worktrees that are idle AND clean so the
    // working-copy disk stays bounded. Clean-gated, so it never discards
    // uncommitted work, and a reaped worktree's branch ref survives. Ide-only:
    // no other role has the worktrees on local disk.
    if crate::server::role_manifest::current_process_role()
        == crate::server::role_manifest::Role::Ide
    {
        crate::server::worktree_registry::spawn_worktree_reaper(
            crate::server::worktree_registry::registry(),
            agentic_state.shutdown_token.clone(),
        );
    }

    // Camera ingest write-coalescing flusher: buffers high-frequency
    // events/health and flushes one batched INSERT per (tenant, stream) per
    // window, so airhouse sees ~one DuckLake commit/window instead of one per
    // edge POST. No-op (synchronous writes) under
    // OXY_CAMERAS_INGEST_BUFFER_DISABLED. Same shutdown token as the other
    // camera loops (final-drains on SIGTERM).
    oxy_cameras::service::ingest_buffer::spawn(agentic_state.shutdown_token.clone());

    // Bridge camera domain events (compliance ingest, health transitions)
    // onto the world-model SSE bus.
    oxy_cameras::service::events::set_sink(Box::new(
        crate::server::api::world_model::publish_camera_domain_event,
    ));

    // World-model live feed. Publishers append to `world_model_events`; this
    // tailer is what turns those rows back into SSE frames on THIS pod, so it
    // must run everywhere — the publishing pod included. Without it a webhook
    // that lands on a serve replica reaches nobody, which is the bug it exists
    // to close. The reaper trims the retained window.
    crate::server::api::world_model::spawn_world_model_tailer();
    crate::server::api::world_model::spawn_world_model_reaper();

    let app_state = AppState {
        enterprise,
        internal: false,
        mode,
        observability,
        startup_cwd,
        preagg_cache,
        preagg_renewal_threshold_secs,
        agentic_state: Some(agentic_state.clone()),
        semantic_layer_cache: super::workspace_cache::new_semantic_layer_cache(),
        semantic_engine_cache: super::workspace_cache::new_semantic_engine_cache(),
    };

    // Built here rather than at the merge below because `install_declarations`
    // sets a OnceLock: there is exactly one install, and a declaration that
    // misses it is decorative. The public tree's own routes have to be in the
    // vector that install receives.
    let (public_router, mut public_decls, _) = build_public_routes(&app_state).into_parts();
    public_decls.extend(
        extra_public_decls
            .iter()
            .map(|d| (d.method, d.path.to_string(), d.role)),
    );
    let public_decls = crate::server::role_manifest::api_prefixed(public_decls);
    let public_router = public_router.merge(extra_public_routes);

    let protected_routes = match mode {
        ServeMode::Cloud => {
            // Billing applies to cloud mode only. The reconciliation job no-ops
            // when Stripe isn't configured (STRIPE_SECRET_KEY absent), so the
            // spawn is always safe. Spawning once at router construction keeps
            // it tied to server lifetime without adding a separate hook.
            // Disabled for now; will re-enable later.
            // spawn_billing_reconciler().await;
            //
            // Surface crates mounted by the composition root (`oxy-server`) —
            // e.g. `oxy-api-github`, `oxy-api-partner-console`, `oxy-api-onboarding`
            // — are merged into the protected tree HERE, before `apply_middleware`,
            // so they inherit the exact standard auth stack (auth / api-key /
            // timeout / publish-token-scope) rather than re-applying it themselves
            // (a reproduction that could drift into an auth bypass). A surface
            // still re-applies its own inner middleware (e.g. github's org routes
            // carry org_middleware + subscription_guard).
            //
            // `extra_api_decls` is what keeps the seam honest: most of these mount
            // Postgres-only routes and the FleetOk default is right for them, but
            // onboarding clones a checkout onto node-local disk, so it declares.
            let (routes, decls) = build_protected_routes(
                app_state.clone(),
                agentic_state.clone(),
                extra_workspace_routes,
                extra_workspace_decls,
            );
            // `build_protected_routes` already returned these `/api`-prefixed;
            // a seam's declarations are relative to the same tree, so they need
            // the same prefix or `classify` never matches them and the route
            // silently takes the FleetOk default.
            let mut decls = decls;
            decls.extend(crate::server::role_manifest::api_prefixed(
                extra_api_decls
                    .iter()
                    .map(|d| (d.method, d.path.to_string(), d.role))
                    .collect(),
            ));
            decls.extend(public_decls.clone());
            apply_middleware(routes.merge(extra_api_routes), decls)?
        }
        ServeMode::Local => {
            let (routes, mut decls) = build_local_protected_routes(
                app_state.clone(),
                agentic_state.clone(),
                extra_workspace_routes,
                extra_workspace_decls,
            );
            decls.extend(public_decls.clone());
            apply_local_middleware(routes, decls)?
        }
    };

    // Camera-fleet routes split across two mounting points:
    //   - Edge /control/* tree mounts here at the top level; the
    //     device-token middleware (inside oxy_cameras::routes::router)
    //     resolves workspace_id from the bearer.
    //   - Operator workspace tree (cameras/edge-boxes, cameras/{id}/zones,
    //     integrations/unifi/*) is merged into build_workspace_routes,
    //     so it sits behind workspace_middleware + auth_middleware and
    //     trusts the URL's workspace_id.
    let camera_routes = oxy_cameras::routes::router::<AppState>(agentic_state.db.clone());
    let app_routes = public_router.merge(protected_routes).merge(camera_routes);

    // External API surface (`/external/api`): curated routes, API-key-only
    // auth, wide-open CORS. Built from the SAME shared `app_state` +
    // `agentic_state` (never a second AgenticState) and returned separately so
    // the caller mounts it OUTSIDE the global `build_cors_layer` — that's what
    // lets its own permissive CORS govern preflight for these routes.
    let external_router = build_external_api_router(app_state.clone(), agentic_state, mode);

    // The Layer-1 cache is handed back so the caller can reach the surfaces it
    // mounts OUTSIDE this router — today the `/customer-apps/{*path}` serve
    // route, whose Oxy Functions run `ctx.semantic` through the same
    // preagg-aware compile every other semantic surface uses. It is the same
    // `Arc` `AppState` carries, not a second cache: the read side and the
    // rebuild side must observe each other's writes.
    let preagg = PreaggCacheCtx {
        cache: app_state.preagg_cache.clone(),
        renewal_threshold_secs: app_state.preagg_renewal_threshold_secs,
    };
    Ok((
        finalize_router(app_routes, app_state),
        external_router,
        preagg,
    ))
}

/// 6-hour background loop that reconciles Stripe seat quantity for every
/// paid org, catching any drift between member counts and what we last sent
/// to Stripe. Idempotent; silently does nothing if Stripe isn't configured.
#[allow(dead_code)] // Disabled for now; will re-enable later.
async fn spawn_billing_reconciler() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        // Already spawned (e.g. tests or duplicated router builds).
        return;
    }
    let Ok(svc) = crate::api::billing::billing_service().await else {
        tracing::debug!("billing reconciler not spawned — Stripe isn't configured");
        return;
    };
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(6 * 60 * 60));
        // Skip the immediate tick — reconciliation at boot is redundant with
        // the live sync that just fired during any recent member change.
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(e) = svc.reconcile_all_seats().await {
                tracing::warn!(?e, "billing seat reconciliation failed");
            }
        }
    });
}

pub async fn internal_api_router(
    enterprise: bool,
    observability: Option<std::sync::Arc<dyn oxy_observability::ObservabilityStore>>,
    shutdown_token: CancellationToken,
    // The internal API mirrors the main app (see below), so it takes the same
    // surface seams `api_router` does. Their declarations are deliberately
    // unused except the workspace seam's (which `build_protected_routes`
    // consumes and this build discards): the primary router owns the
    // declaration registry, and installing a second one from here would race it.
    seams: SurfaceSeams,
) -> Result<Router, OxyError> {
    let SurfaceSeams {
        api:
            SurfaceSeam {
                routes: extra_api_routes,
                decls: _extra_api_decls,
            },
        workspace:
            SurfaceSeam {
                routes: extra_workspace_routes,
                decls: extra_workspace_decls,
            },
        public:
            SurfaceSeam {
                routes: extra_public_routes,
                decls: _extra_public_decls,
            },
    } = seams;
    let app_state = AppState {
        enterprise,
        internal: true,
        mode: ServeMode::Cloud,
        observability,
        startup_cwd: std::path::PathBuf::new(),
        preagg_cache: None,
        preagg_renewal_threshold_secs: None,
        // Internal router has no custom-app endpoints; agentic state
        // not needed and explicitly omitted.
        agentic_state: None,
        semantic_layer_cache: super::workspace_cache::new_semantic_layer_cache(),
        semantic_engine_cache: super::workspace_cache::new_semantic_engine_cache(),
    };
    // `api_router` owns startup cleanup + recovery for the whole process;
    // the internal router shares the same database state, so it skips both
    // to avoid racing with the primary recovery task on the same runs.
    let agentic_state = new_agentic_state(shutdown_token, false).await?;
    spawn_shutdown_hook(agentic_state.clone());

    // The internal API is an internal-auth MIRROR of the main app, so it mounts
    // the SAME extracted surface crates (via the seams) that `api_router` does.
    // The agentic browser tests drive this port; a surface merged only into
    // `api_router` 404/405s on :3001 while working on the public port — e.g.
    // onboarding's `POST /orgs/{org_id}/onboarding/new`. `extra_api_routes`
    // merges BEFORE the layers so it inherits internal auth.
    //
    // The primary `api_router` owns the declaration registry, so this build
    // discards its copy rather than racing to install a second one.
    let (protected_routes, _decls) = build_protected_routes(
        app_state.clone(),
        agentic_state,
        extra_workspace_routes,
        extra_workspace_decls,
    );
    let protected_routes = protected_routes
        .merge(extra_api_routes)
        .layer(middleware::from_fn(timeout_middleware))
        .layer(middleware::from_fn(internal_auth_middleware));

    let app_routes = build_public_routes(&app_state)
        .into_router()
        .merge(extra_public_routes)
        .merge(protected_routes);

    Ok(finalize_router(app_routes, app_state))
}

async fn new_agentic_state(
    shutdown_token: CancellationToken,
    run_cleanup: bool,
) -> Result<Arc<AgenticState>, OxyError> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("db connect failed: {e}")))?;
    if run_cleanup {
        cleanup_stale_runs(&db).await.ok();
    }
    let thread_owner: Arc<dyn agentic_pipeline::platform::ThreadOwnerLookup> =
        Arc::new(crate::agentic_wiring::OxyThreadOwnerLookup::new(db.clone()));

    // Cross-process wake source. The factory abstracts over both
    // password and IAM auth modes — same OXY_DATABASE_AUTH_MODE env
    // var that selects the connection pool's auth path. IAM mode
    // mints a fresh SigV4 token on every (re)connect via the
    // factory closure; no separate refresh loop needed (the listener
    // holds one connection at a time and Postgres doesn't re-auth
    // mid-stream).
    let factory = crate::server::task_router_listener::listener_factory_from_env()
        .map_err(|e| OxyError::RuntimeError(format!("listener factory: {e}")))?;
    // Match the connection pool's TLS strictness (OXY_DATABASE_SSL_MODE) so
    // the listener's handshake succeeds where the pool's does — RDS and
    // CloudNativePG CAs aren't in the Mozilla bundle.
    let tls_verification =
        crate::server::task_router_listener::listener_tls_verification_from_env()
            .map_err(|e| OxyError::RuntimeError(format!("listener tls mode: {e}")))?;
    // Chat's cross-replica delivery listener rides the SAME factory and TLS
    // posture. Two people in one channel are routinely on different replicas,
    // so an in-process bus would deliver a message to whoever happened to hit
    // the replica that accepted the POST and to nobody else — invisible on a
    // dev box, total in production.
    crate::server::api::chat::delivery::start_listener(
        factory.clone(),
        tls_verification,
        shutdown_token.clone(),
    );

    let (router_handle, router_cancel) =
        agentic_runtime::router::PostgresTaskRouter::start_with_options(
            db.clone(),
            factory,
            agentic_runtime::router::PostgresTaskRouterOptions {
                tls_verification,
                ..Default::default()
            },
        );
    let router: Arc<dyn agentic_runtime::router::TaskRouter> = router_handle;
    // Tie the listener's lifetime to the same shutdown token as the
    // rest of the agentic state. Dropped CancellationToken would
    // auto-cancel only on the last clone going away; explicit
    // cancel-on-shutdown is clearer.
    let shutdown_for_router = shutdown_token.clone();
    tokio::spawn(async move {
        shutdown_for_router.cancelled().await;
        router_cancel.cancel();
    });
    tracing::info!(
        target: "agentic",
        keepalive_secs = agentic_runtime::router::DEFAULT_LISTENER_KEEPALIVE_INTERVAL.as_secs(),
        "task router: PostgresTaskRouter (LISTEN/NOTIFY)"
    );

    // Process-level background jobs (reaper today; matcher health
    // probe later). Tied to the same shutdown token as the rest of
    // the agentic state so the loop exits on Ctrl-C / SIGTERM with
    // everything else.
    //
    // "Process-level" overstates it: both callers of `new_agentic_state`
    // reach here, and a single `oxy serve` builds both routers — the
    // public one always, the internal one unless `internal_port=0`
    // (default 3001). So a default serve process runs *two* of these
    // loops, i.e. two reapers, two health probes, two retention sweeps.
    // Benign — a reap claims its rows with an `UPDATE`, so the second
    // loop finds nothing rather than double-dead-lettering, and both
    // increment the same in-process counters — but note the asymmetry
    // with `internal_api_router`'s call site above: there, startup
    // cleanup and recovery are deliberately skipped on the internal
    // router to avoid exactly this kind of duplication, and these loops
    // are not. That looks unintended rather than decided; collapsing
    // them wants its own change (and a test), not a rider on a doc pass.
    let bg_cancel = agentic_runtime::background::start(db.clone(), router.clone());
    let shutdown_for_bg = shutdown_token.clone();
    tokio::spawn(async move {
        shutdown_for_bg.cancelled().await;
        bg_cancel.cancel();
    });

    Ok(Arc::new(
        AgenticState::new(shutdown_token, db, thread_owner)
            .with_builder_test_runner(Arc::new(OxyTestRunner))
            .with_builder_app_runner(Arc::new(OxyBuilderAppRunner))
            .with_router(router),
    ))
}

/// Applies the shared outer layers — state, CORS, the 60-second global
/// timeout (aligned with load-balancer limits; individual sync endpoints
/// have their own tighter timeouts), and Sentry request tracing.
fn finalize_router(app_routes: Router<AppState>, app_state: AppState) -> Router {
    let global_timeout =
        TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, Duration::from_secs(60));

    // role_middleware is applied at the OUTER router in serve.rs::main, not
    // here — the api_router we return from finalize_router gets nested at
    // /api on the outer router, but the IDE-page `/ide*` routes are served
    // by the static `fallback_service` on that same outer router. Wrapping
    // enforce_role here means it would never fire on /ide requests, which
    // are exactly what OXY_ROLE=serve must refuse.
    app_routes
        .with_state(app_state)
        .layer(build_cors_layer())
        .layer(global_timeout)
        .layer(ServiceBuilder::new().layer(NewSentryLayer::<Request<Body>>::new_from_top()))
}
