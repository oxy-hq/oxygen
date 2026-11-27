//! Public entry points that assemble the full router.
//!
//! [`api_router`] is the user-facing API (cloud or local, driven by
//! [`ServeMode`]). [`internal_api_router`] is the internal port — always
//! cloud-shape, protected by an API-key-only middleware.
//!
//! The builders only build. The background work a serve process runs — the
//! run drivers, the shutdown hooks, the surfaces' jobs — is started by `oxy
//! serve` from the [`AgenticState`] they hand back (decompose BL-W,
//! 2026-11-23); `boot_guard_tests` fails if a builder starts any of it again.

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

use crate::router::middlewares::timeout::timeout_middleware;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;
use oxy_app_core::serve_mode::ServeMode;

use super::protected::{
    apply_local_middleware, apply_middleware, build_external_api_router,
    build_local_protected_routes, build_protected_routes,
};
use super::public::build_public_routes;
use super::{AppState, build_cors_layer};
use oxy_app_core::seams::{SurfaceSeam, SurfaceSeams};

/// Builds the main API router (mounted under `/api`) and, alongside it, the
/// external API router (`/external/api`). The external one is returned
/// separately so the caller can mount it OUTSIDE the global CORS layer; see
/// [`build_external_api_router`].
///
/// Starts no background work: [`api_router_with_state`] without the state.
/// `oxy serve` calls that one and starts the boot jobs from the state.
pub async fn api_router(
    mode: ServeMode,
    enterprise: bool,
    observability: Option<std::sync::Arc<dyn oxy_observability::ObservabilityStore>>,
    startup_cwd: std::path::PathBuf,
    shutdown_token: CancellationToken,
    disable_inprocess_workers: bool,
    seams: SurfaceSeams,
) -> Result<(Router, Router, PreaggCacheCtx), OxyError> {
    let (router, external_router, preagg, _agentic_state) = api_router_with_state(ApiRouterArgs {
        mode,
        enterprise,
        observability,
        startup_cwd,
        shutdown_token,
        disable_inprocess_workers,
        seams,
    })
    .await?;
    Ok((router, external_router, preagg))
}

/// [`api_router`]'s arguments, gathered into one value for
/// [`api_router_with_state`].
pub struct ApiRouterArgs {
    pub mode: ServeMode,
    pub enterprise: bool,
    pub observability: Option<std::sync::Arc<dyn oxy_observability::ObservabilityStore>>,
    pub startup_cwd: std::path::PathBuf,
    pub shutdown_token: CancellationToken,
    /// `--no-workers`: skips the boot cleanup in `new_agentic_state` here, and
    /// the startup pass and global driver in `serve`'s boot.
    pub disable_inprocess_workers: bool,
    /// Surface routes composed by the caller (the top `oxy-server` crate), by
    /// which side of the auth stack each lands on — see [`SurfaceSeams`]. The
    /// api seam joins the protected tree BEFORE `apply_middleware` (cloud mode
    /// only: local mode never mounts the org/global tree and drops it); the
    /// workspace seam goes INSIDE the `/{workspace_id}` nest; the public seam
    /// merges beside `build_public_routes` in BOTH modes, with no auth layer.
    pub seams: SurfaceSeams,
}

/// The router [`api_router`] builds, plus the [`AgenticState`] it was built
/// over, which `oxy serve` starts its boot jobs from (the run drivers, the
/// shutdown hook, the surfaces' background and fleet jobs). Like
/// [`api_router`], it starts none of them.
pub async fn api_router_with_state(
    args: ApiRouterArgs,
) -> Result<(Router, Router, PreaggCacheCtx, Arc<AgenticState>), OxyError> {
    let ApiRouterArgs {
        mode,
        enterprise,
        observability,
        startup_cwd,
        shutdown_token,
        disable_inprocess_workers,
        seams,
    } = args;
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
        previews:
            SurfaceSeam {
                routes: preview_routes,
                decls: preview_decls,
            },
        public:
            SurfaceSeam {
                routes: extra_public_routes,
                decls: extra_public_decls,
            },
        external: extra_external_routes,
        catalog,
        admin,
        admin_nests,
        // Started by `serve`'s boot (`oxy-server`'s `commands::serve_background`),
        // which takes them off the seams before handing them here.
        tasks: _,
        background: _,
        fleet_jobs: _,
        global_ticks: _,
        // Merged by `serve`, which takes it before handing the seams here.
        outer: _,
        outer_decls,
        function_runtime,
        // Run by `oxy migrate` and `oxy dev seed`, which never build this router.
        post_migrate: _,
        example_apps: _,
        // Documentation only — `build_openapi_doc` merges it, not the router.
        openapi: _,
        // Run by `serve` and `worker` at boot, around migrations.
        notifications: _,
    } = seams;
    // Boot cleanup only where the startup pass runs: a `serve` replica
    // (`--no-workers`) resumes nothing, so every row it would reconcile belongs
    // to another process — and it boots on every deploy and scale-up.
    let agentic_state = new_agentic_state(shutdown_token, !disable_inprocess_workers).await?;

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

    // The boot jobs — the global ticks, the run drivers, the shutdown hook,
    // compile maintenance, the audit prune, the token-usage flush, the
    // surfaces' background and fleet jobs and the camera loops — are not
    // started here. `oxy serve` starts them once this returns, from the state it
    // hands back (`oxy-server`'s `commands::serve_background`), so a router
    // build boots nothing; `boot_guard_tests` holds the line.

    let app_state = AppState {
        enterprise,
        internal: false,
        mode,
        observability,
        startup_cwd,
        preagg_cache,
        preagg_renewal_threshold_secs,
        agentic_state: Some(agentic_state.clone()),
        semantic_layer_cache: oxy_app_core::workspace_cache::new_semantic_layer_cache(),
        semantic_engine_cache: oxy_app_core::workspace_cache::new_semantic_engine_cache(),
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
    let public_decls = crate::role_manifest::api_prefixed(public_decls);
    let public_router = public_router.merge(extra_public_routes);

    let protected_routes = match mode {
        ServeMode::Cloud => {
            // Billing applies to cloud mode only. The reconciliation job no-ops
            // when Stripe isn't configured (STRIPE_SECRET_KEY absent), so the
            // spawn is always safe. Spawning once at router construction keeps
            // it tied to server lifetime without adding a separate hook.
            // Disabled for now; will re-enable later. (The reconciler is
            // `oxy-server`'s `drivers::billing_reconciler` since decompose BL1.)
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
                preview_routes,
                preview_decls,
                catalog,
                admin,
                admin_nests,
            );
            // `build_protected_routes` already returned these `/api`-prefixed;
            // a seam's declarations are relative to the same tree, so they need
            // the same prefix or `classify` never matches them and the route
            // silently takes the FleetOk default.
            let mut decls = decls;
            decls.extend(crate::role_manifest::api_prefixed(
                extra_api_decls
                    .iter()
                    .map(|d| (d.method, d.path.to_string(), d.role))
                    .collect(),
            ));
            decls.extend(public_decls.clone());
            // `/customer-apps/{*path}` is mounted on the OUTER router in serve.rs,
            // not in this tree, but this is the declaration set that gets
            // installed — so the split its crate declares rides along rather than
            // living in a table. Absolute: no `/api` prefix.
            decls.extend(
                outer_decls
                    .iter()
                    .map(|d| (d.method, d.path.to_string(), d.role)),
            );
            apply_middleware(routes.merge(extra_api_routes), decls)?
        }
        ServeMode::Local => {
            // Workspace previews are staff-only and cloud-only; local mode drops
            // the seam, as it did the in-tree mount.
            drop((preview_routes, preview_decls));
            let (routes, mut decls) = build_local_protected_routes(
                app_state.clone(),
                agentic_state.clone(),
                extra_workspace_routes,
                extra_workspace_decls,
                catalog,
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
    // The function runtime, for the handlers in this crate that run a function
    // on a person's behalf (a notification's function action). Handed in by the
    // composition root; absent, those answer that no runtime is available.
    let app_routes = match function_runtime {
        Some(runtime) => app_routes.layer(axum::Extension(runtime)),
        None => app_routes,
    };

    // External API surface (`/external/api`): curated routes, API-key-only
    // auth, wide-open CORS. Built from the SAME shared `app_state` +
    // `agentic_state` (never a second AgenticState) and returned separately so
    // the caller mounts it OUTSIDE the global `build_cors_layer` — that's what
    // lets its own permissive CORS govern preflight for these routes.
    let external_router = build_external_api_router(
        app_state.clone(),
        agentic_state.clone(),
        mode,
        extra_external_routes,
    );

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
        agentic_state,
    ))
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
) -> Result<(Router, Arc<AgenticState>), OxyError> {
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
        previews:
            SurfaceSeam {
                routes: preview_routes,
                decls: preview_decls,
            },
        public:
            SurfaceSeam {
                routes: extra_public_routes,
                decls: _extra_public_decls,
            },
        // The internal router mounts no external tree.
        external: _,
        catalog,
        admin,
        admin_nests,
        // The internal router starts no jobs, drives no queue and mounts no
        // outer route.
        tasks: _,
        background: _,
        fleet_jobs: _,
        global_ticks: _,
        outer: _,
        outer_decls: _,
        function_runtime: _,
        post_migrate: _,
        example_apps: _,
        // Documentation only — `build_openapi_doc` merges it, not the router.
        openapi: _,
        notifications: _,
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
        semantic_layer_cache: oxy_app_core::workspace_cache::new_semantic_layer_cache(),
        semantic_engine_cache: oxy_app_core::workspace_cache::new_semantic_engine_cache(),
    };
    // `api_router` owns startup cleanup, and `serve`'s boot recovery, for the
    // whole process; the internal router shares the same database state, so it
    // skips both to avoid racing with the primary recovery task on the same
    // runs. Its shutdown hook is `serve`'s to spawn, from the state returned.
    let agentic_state = new_agentic_state(shutdown_token, false).await?;

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
        agentic_state.clone(),
        extra_workspace_routes,
        extra_workspace_decls,
        preview_routes,
        preview_decls,
        catalog,
        admin,
        admin_nests,
    );
    let protected_routes = protected_routes
        .merge(extra_api_routes)
        .layer(middleware::from_fn(timeout_middleware))
        .layer(middleware::from_fn(internal_auth_middleware));

    let app_routes = build_public_routes(&app_state)
        .into_router()
        .merge(extra_public_routes)
        .merge(protected_routes);

    Ok((finalize_router(app_routes, app_state), agentic_state))
}

async fn new_agentic_state(
    shutdown_token: CancellationToken,
    run_cleanup: bool,
) -> Result<Arc<AgenticState>, OxyError> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("db connect failed: {e}")))?;
    if run_cleanup && let Err(e) = cleanup_stale_runs(&db).await {
        // Not fatal — a row the cleanup did not reach is simply left as it
        // was, which is what every `oxy worker` boot leaves too. But the
        // cleanup stops at its first failed read rather than fail a run on a
        // guess, so a boot that stopped early has to say so.
        tracing::warn!(
            error = %e,
            "startup stale-run cleanup stopped early; the rows it did not reach are unchanged"
        );
    }
    let thread_owner: Arc<dyn agentic_pipeline::platform::ThreadOwnerLookup> =
        Arc::new(oxy_workspace_runtime::thread_owner::OxyThreadOwnerLookup::new(db.clone()));

    // Cross-process wake source. The factory abstracts over both
    // password and IAM auth modes — same OXY_DATABASE_AUTH_MODE env
    // var that selects the connection pool's auth path. IAM mode
    // mints a fresh SigV4 token on every (re)connect via the
    // factory closure; no separate refresh loop needed (the listener
    // holds one connection at a time and Postgres doesn't re-auth
    // mid-stream).
    let factory = oxy_app_core::listener::listener_factory_from_env()
        .map_err(|e| OxyError::RuntimeError(format!("listener factory: {e}")))?;
    // Match the connection pool's TLS strictness (OXY_DATABASE_SSL_MODE) so
    // the listener's handshake succeeds where the pool's does — RDS and
    // CloudNativePG CAs aren't in the Mozilla bundle.
    let tls_verification = oxy_app_core::listener::listener_tls_verification_from_env()
        .map_err(|e| OxyError::RuntimeError(format!("listener tls mode: {e}")))?;
    // Chat's cross-replica delivery listener rides the same factory and TLS
    // posture, built from the same environment (`oxy_app_core::listener`). It is
    // `oxy-api-threads`' since 2026-11-16 and starts with the surfaces'
    // background jobs in `api_router` (`chat_delivery_listener`, which
    // `SurfaceSeams::require_custom_apps` refuses to boot without).
    let (router_handle, router_cancel) =
        agentic_runtime::router::PostgresTaskRouter::start_with_options(
            db.clone(),
            factory,
            agentic_runtime::router::PostgresTaskRouterOptions {
                tls_verification,
                ..Default::default()
            },
        );
    // Kept concrete for one more call below: the run-events fan-out is bound
    // to the state this function is about to build.
    let stream_waker_router = Arc::clone(&router_handle);
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

    let state = Arc::new(
        agentic_http::new_agentic_state(shutdown_token, db, thread_owner)
            .with_router(router)
            .with_queued_starts(true),
    );
    // A run driven on another pod appends to `agentic_run_events` and NOTIFYs
    // `oxy_run_events`; the listener above hands the run id to this state's
    // `remote_wakes`, where `GET /runs/:id/events` parks for such a run. The
    // state did not exist when the router started, hence the late bind; the
    // stream's safety-net timer covers the moments in between. The `oxy
    // worker` process serves no streams, so `WorkerRuntime` does not bind.
    let stream_waker: Arc<dyn agentic_runtime::router::RunEventWaker> =
        state.runtime.remote_wakes.clone();
    stream_waker_router.bind_run_event_waker(stream_waker);
    Ok(state)
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

#[cfg(test)]
#[path = "entry_boot_guard_tests.rs"]
mod boot_guard_tests;
