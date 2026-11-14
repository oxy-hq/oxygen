//! The custom-app work this composition root hands `oxy-app` through
//! [`SurfaceSeams`](oxy_app::server::router::SurfaceSeams): the seven task kinds
//! the worker fleet runs, the process-lifetime background jobs, the outer
//! `/customer-apps/{*path}` mount, the function runtime for the `oxy-app`
//! handlers that run a function, and the `oxy migrate` and `oxy seed` halves.
//!
//! The executors, the mount, the runtime and the seed are `oxy-api-custom-apps`'s;
//! the usage-retention and usage-drain jobs and the storage sweeps are in
//! `oxy-custom-apps`. What is here is the decision that a process runs each of
//! them. `oxy-app`'s router and `serve` name none of it.
//!
//! `oxy-app` refuses to start (`SurfaceSeams::require_custom_apps`) when a kind
//! or a job it needs is missing from what this module returns, so dropping an
//! entry is a boot error, never a task that queues and is never run.

use std::sync::Arc;

use axum::Router;
use axum::routing::any;
use oxy_api_custom_apps::app_function_executor::{APP_FUNCTION_KIND, AppFunctionTaskExecutor};
use oxy_api_custom_apps::custom_apps_env_branch::task as env_branch_task;
use oxy_api_custom_apps::custom_apps_nonproduction::{staging_task, staging_task_executor};
use oxy_api_custom_apps::custom_apps_sandboxes::{
    maintenance, migrations_task, oltp_task, teardown,
};
use oxy_api_custom_apps::custom_apps_usage_backfill::{self, USAGE_BACKFILL_KIND};
use oxy_api_custom_apps::data_plane::query::DataPlaneQueryExecutor;
use oxy_api_custom_apps::serve as custom_apps_serve;
use oxy_app::server::api::middlewares::workspace_context::PreaggCacheCtx;
use oxy_app::server::compile_reconcile::env_pinner::EnvPinner;
use oxy_app::server::router::{BackgroundSeam, TaskSeam};
use oxy_app_core::function_seam::{FunctionQueryExecutor, FunctionRuntime};
use oxy_app_core::semantic_pin_seam::SemanticPinner;
use oxy_custom_apps::storage::{retention, sweeper};
use oxy_custom_apps::{tracking as custom_apps_tracking, usage_queue as custom_apps_usage_queue};
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::compression::predicate::{DefaultPredicate, NotForContentType, Predicate};

/// The task kinds a worker runs for custom apps, each with the executor that
/// runs it.
pub(crate) fn tasks() -> Vec<TaskSeam> {
    vec![
        // A custom app's ask: every live one is queued by its start
        // (`agent_ask::start`). It builds its own context from the caller the
        // request authenticated — read off the run row — never from this
        // driver's subject-less platform, and shares the driver's schema cache
        // with the chat runs beside it.
        TaskSeam {
            kind: oxy_api_custom_apps::agent_ask::task::AGENT_ASK_KIND,
            build: |deps| {
                Arc::new(oxy_api_custom_apps::agent_ask::executor::AgentAskExecutor {
                    db: deps.db.clone(),
                    schema_cache: deps.schema_cache.cloned(),
                })
            },
        },
        // A scheduled or manual Oxy Function run. Its `ctx.semantic` resolves
        // rollups on the node draining the queue, the same way its
        // HTTP-invoked twin does.
        TaskSeam {
            kind: APP_FUNCTION_KIND,
            build: |deps| {
                Arc::new(AppFunctionTaskExecutor {
                    db: deps.db.clone(),
                    preagg: deps.preagg.clone(),
                    // The worker's own composition root: the shared data-plane
                    // query executor, as a trait object, so the function module
                    // never imports `data_plane::query`.
                    executor: Arc::new(DataPlaneQueryExecutor) as Arc<dyn FunctionQueryExecutor>,
                })
            },
        },
        // Staging migrations, queued by every publish: the sibling Airhouse
        // schema and the org's OLTP staging branch, one task per store.
        TaskSeam {
            kind: staging_task::STAGING_MIGRATIONS_KIND,
            build: |deps| {
                Arc::new(staging_task_executor::StagingMigrationsExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // A sandbox (`dev-<handle>`) torn down after a delete or an expiry.
        TaskSeam {
            kind: teardown::SANDBOX_TEARDOWN_KIND,
            build: |deps| {
                Arc::new(teardown::SandboxTeardownExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // A sandbox build's Airhouse migrations. A kind of its own rather than
        // a field on the staging task: a worker that does not know it fails the
        // task instead of applying a sandbox's files to staging's sibling.
        TaskSeam {
            kind: migrations_task::SANDBOX_MIGRATIONS_KIND,
            build: |deps| {
                Arc::new(migrations_task::SandboxMigrationsExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // A sandbox's own schema on the org's OLTP staging branch.
        TaskSeam {
            kind: oltp_task::SANDBOX_OLTP_KIND,
            build: |deps| {
                Arc::new(oltp_task::SandboxOltpExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // A sandbox's own OLTP branch, cut, reset and migrated (`planes.oltp`
        // `branch` or `empty`).
        TaskSeam {
            kind: env_branch_task::ENV_OLTP_BRANCH_KIND,
            build: |deps| {
                Arc::new(env_branch_task::EnvBranchExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // A workspace's custom-app usage history copied from Postgres into its
        // Airhouse, queued by the backfill sweep below. `oxy-app` queues none
        // of these, so the kind is not in `CUSTOM_APP_TASK_KINDS`.
        TaskSeam {
            kind: USAGE_BACKFILL_KIND,
            build: |deps| {
                Arc::new(custom_apps_usage_backfill::UsageBackfillExecutor {
                    db: deps.db.clone(),
                })
            },
        },
    ]
}

/// Runs a function request for the `oxy-app` handlers that do on a person's
/// behalf (a notification's function action).
pub(crate) fn function_runtime() -> Arc<dyn FunctionRuntime> {
    Arc::new(oxy_api_custom_apps::function_runtime::Runtime::new(
        Arc::new(DataPlaneQueryExecutor),
    ))
}

/// The staff console's publish and unpublish, for the partner console to call.
pub(crate) fn app_lifecycle() -> Arc<dyn oxy_app_core::app_lifecycle::AppLifecycle> {
    Arc::new(oxy_api_custom_apps::app_lifecycle::Lifecycle)
}

/// The pin a planes write calls to point an environment's semantic plane at a
/// workspace branch (`PUT …/planes`, `POST …/environments`). The routes are
/// `oxy-api-custom-apps`'s and cannot name `oxy-app`, which holds the branch
/// lookup, the compile and the pin columns; they find this on the request as an
/// `Extension`, and answer `503` to a `branch:` choice when it is missing.
pub(crate) fn semantic_pinner() -> Arc<dyn SemanticPinner> {
    Arc::new(EnvPinner)
}

/// What `oxy migrate` prints after the migrations: the functions preflight.
pub(crate) fn post_migrate() -> oxy_app::server::router::MigrateReport {
    |build| {
        Box::pin(oxy_api_custom_apps::custom_apps_functions::preflight::run(
            build,
        ))
    }
}

/// The jobs a serve process starts once and leaves running for custom apps.
pub(crate) fn background() -> Vec<BackgroundSeam> {
    vec![
        // Verify, once, that the asset bucket enforces the TTL classes this
        // binary stamps on objects. Singleton-gated: the answer is the same from
        // every replica.
        BackgroundSeam {
            name: "custom_app_asset_lifecycle_verify",
            start: |deps| {
                retention::spawn_lifecycle_verify(deps.singleton);
                None
            },
        },
        // Re-measure each app's asset silo into `app_storage_usage`.
        // Singleton-gated, or every replica would multiply the LIST cost.
        BackgroundSeam {
            name: "custom_app_storage_sweeper",
            start: |deps| {
                sweeper::spawn_periodic_sweep(
                    deps.db.clone(),
                    deps.shutdown.clone(),
                    deps.singleton,
                );
                None
            },
        },
        // Expire sandboxes idle past their TTL and queue again the teardowns
        // that did not finish. Only writes rows; two replicas sweeping at once
        // is harmless.
        BackgroundSeam {
            name: "custom_app_sandbox_maintenance",
            start: |_| {
                maintenance::spawn(maintenance::MaintenanceConfig::from_env());
                None
            },
        },
        // The 90-day retention sweep for page-view and event rows.
        BackgroundSeam {
            name: "custom_app_tracking_retention",
            start: |_| {
                custom_apps_tracking::spawn_retention_cleanup();
                None
            },
        },
        // The one task that batch-inserts the usage rows the serve path
        // `try_send`s onto a bounded queue. It flushes this process's memory
        // once more on shutdown, so its handle is returned: the router
        // registers it as a shutdown hook.
        BackgroundSeam {
            name: "custom_app_usage_drain",
            start: |deps| {
                Some(custom_apps_usage_queue::spawn_usage_drain(
                    deps.shutdown.clone(),
                ))
            },
        },
        // The custom-app log tail's listener rides the task router's connection
        // settings, for the same reason chat's does. Installed, not started: the
        // connection opens on this process's first tail, and most processes never
        // hold one.
        BackgroundSeam {
            name: "custom_app_tail_listener",
            start: |deps| {
                use oxy_app::surface::listener::{
                    listener_factory_from_env, listener_tls_verification_from_env,
                };
                match (
                    listener_factory_from_env(),
                    listener_tls_verification_from_env(),
                ) {
                    (Ok(factory), Ok(tls)) => {
                        oxy_api_custom_apps::custom_apps_tail::listener::install(
                            factory,
                            tls,
                            deps.shutdown.clone(),
                        )
                    }
                    (Err(e), _) | (_, Err(e)) => {
                        tracing::warn!(error = %e, "custom-app log tail listener not installed")
                    }
                }
                None
            },
        },
        // Queue the usage backfill for each workspace that has an Airhouse and
        // is not yet marked finished. Singleton-gated: two replicas sweeping
        // would only race to queue the same tasks.
        BackgroundSeam {
            name: "custom_app_usage_backfill_sweep",
            start: |deps| {
                custom_apps_usage_backfill::spawn_sweep(
                    deps.db.clone(),
                    deps.shutdown.clone(),
                    deps.singleton,
                );
                None
            },
        },
        // Resolve the function concurrency ceilings and the isolate heap limit at
        // boot, so their gauges read the limit and not 0 until a first call.
        BackgroundSeam {
            name: "custom_app_function_limits",
            start: |_| {
                oxy_api_custom_apps::function_runtime::resolve_limits_at_boot();
                None
            },
        },
    ]
}

/// The custom-app mount's compression policy: `DefaultPredicate` (no streams,
/// no already-encoded bodies, no tiny ones) minus `text/html`, whose brotli form
/// is memoised per build and served by the handler itself.
pub(crate) fn custom_app_compression() -> CompressionLayer<impl Predicate> {
    CompressionLayer::new()
        .compress_when(DefaultPredicate::new().and(NotForContentType::const_new("text/html")))
}

/// The browser-facing `/customer-apps/{*path}` route that `serve` mounts at the
/// top level of the outer router, with the layers a custom app's requests need.
///
/// `preagg_ctx` is the Layer-1 preagg cache the API router owns: this route is
/// mounted outside it and so has no `AppState` to read it from.
pub(crate) fn outer(preagg_ctx: PreaggCacheCtx) -> Router {
    Router::new()
        // Customer-app subpath bundle serving. Mounted at the top level (NOT
        // under /api) because the URL is browser-facing and must redirect to
        // /login on auth failure rather than return 401. The handler
        // authenticates inline and serves the bundle from disk OR (for
        // remote-hosted source) reverse-proxies to the upstream — so we
        // accept every method (POST form submits, Next.js Server Actions,
        // bundle-internal `/api/*` fetches), not just GET.
        //
        // 32 MiB body ceiling to match the proxy's in-handler cap; without
        // this layer axum's 2 MiB default rejects larger requests long
        // before the proxy sees them.
        .route(
            "/customer-apps/{*path}",
            // brotli/gzip for bundle bytes. DefaultPredicate skips
            // text/event-stream (the /fn SSE stream) and already-encoded
            // bodies (the V0 proxy), so streaming is unaffected. `text/html`
            // is excluded: the serve path memoises a brotli copy of each
            // rendered document (`custom_apps_html_cache`) instead of
            // re-compressing it per navigation.
            any(custom_apps_serve::serve_dispatch).layer(
                ServiceBuilder::new()
                    .layer(axum::extract::DefaultBodyLimit::max(32 * 1024 * 1024))
                    .layer(custom_app_compression())
                    // BENEATH compression, not only on the outer router: by the
                    // time a response climbs past `CompressionLayer` its body is
                    // gzip/br with no exact size, and `record_error_body` skips
                    // anything without one — so the outer mount can never see a
                    // custom-app error body over 32 bytes. The request span is
                    // still current here (the trace layer wraps routing).
                    .layer(axum::middleware::from_fn(
                        oxy_telemetry::http_trace::record_error_body,
                    ))
                    // Inject the function query executor (the shared data plane)
                    // so `custom_apps_functions` runs `ctx.query` through the
                    // trait without importing `data_plane::query`.
                    .layer(axum::Extension(std::sync::Arc::new(DataPlaneQueryExecutor)
                        as std::sync::Arc<dyn FunctionQueryExecutor>))
                    // Inject the Layer-1 preagg cache the API router owns, so a
                    // function's `ctx.semantic` compiles through the same
                    // rollup-aware path as `/api/projects/{id}/semantic-query`.
                    // This route is mounted outside the API router and so has no
                    // `AppState` to read it from; the extension is the same seam
                    // the query executor already uses.
                    .layer(axum::Extension(preagg_ctx)),
            ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, header};
    use axum::routing::get;
    use tower::ServiceExt;

    /// HTML is left to the handler's memoised brotli copy; everything else on
    /// the mount is still compressed here.
    #[tokio::test]
    async fn custom_app_compression_skips_html_but_not_other_text() {
        let body = "x".repeat(4096);
        let page = body.clone();
        let app = Router::new()
            .route(
                "/page",
                get(move || {
                    let page = page.clone();
                    async move { ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page) }
                }),
            )
            .route(
                "/x.js",
                get(move || {
                    let body = body.clone();
                    async move { ([(header::CONTENT_TYPE, "application/javascript")], body) }
                }),
            )
            .layer(custom_app_compression());
        let encoding = |path: &'static str| {
            let app = app.clone();
            async move {
                app.oneshot(
                    Request::get(path)
                        .header("accept-encoding", "br")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .headers()
                .get(header::CONTENT_ENCODING)
                .map(|v| v.to_str().unwrap().to_string())
            }
        };
        assert_eq!(
            encoding("/page").await,
            None,
            "html is not re-compressed per request"
        );
        assert_eq!(encoding("/x.js").await.as_deref(), Some("br"));
    }

    /// The task kinds the fleet knew before this list was handed in, plus the
    /// usage backfill and the queued ask (registered in `oxy-app` until its
    /// executor moved to `oxy-api-custom-apps`, 2026-11-14). A kind added here, or dropped, is a change to what a
    /// worker can run, so it is spelled out rather than derived from the
    /// constants it is built from.
    #[test]
    fn the_task_kinds_are_exactly_these_eight() {
        let mut kinds: Vec<&str> = tasks().iter().map(|t| t.kind).collect();
        kinds.sort_unstable();
        assert_eq!(
            kinds,
            [
                "app_function",
                "custom_app_agent_ask",
                "custom_app_env_oltp_branch",
                "custom_app_sandbox_migrations",
                "custom_app_sandbox_oltp",
                "custom_app_sandbox_teardown",
                "custom_app_staging_migrations",
                "custom_app_usage_backfill",
            ]
        );
    }

    /// Same for the background jobs: the five `api_router` and `serve` started
    /// by name before, plus the log tail's listener, the function limits'
    /// boot-time resolution and the usage backfill sweep.
    /// `SurfaceSeams::require_custom_apps` refuses to start without the seven
    /// it names; the sweep is not one of them, because serving does not need it.
    #[test]
    fn the_background_jobs_are_exactly_the_eight_the_composition_root_starts() {
        let mut names: Vec<&str> = background().iter().map(|b| b.name).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "custom_app_asset_lifecycle_verify",
                "custom_app_function_limits",
                "custom_app_sandbox_maintenance",
                "custom_app_storage_sweeper",
                "custom_app_tail_listener",
                "custom_app_tracking_retention",
                "custom_app_usage_backfill_sweep",
                "custom_app_usage_drain",
            ]
        );
    }
}
