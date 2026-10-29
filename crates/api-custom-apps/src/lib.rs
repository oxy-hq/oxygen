//! Custom Apps HTTP surface: everything the custom-app vertical serves, runs and
//! administers. It is one crate so that V8, `sourcemap` and `brotli` link into
//! this crate and the one binary that composes it, not into `oxy-app` and every
//! one of its test binaries.
//!
//! - [`serve`], [`client`], [`beacon`], [`html_cache`], [`sourcemap`]: the
//!   `/customer-apps/{*path}` route that serves a published bundle (and
//!   dispatches `POST …/fn/<name>` to the function runtime), the service worker
//!   and client runtime it injects, and the browser beacon it answers for.
//! - [`health`], [`logs`]: `/customer-apps/{org}/{app}/health`, `/logs`, `/errors`.
//! - [`custom_apps_functions`]: the Oxy Functions host, the V8 runtime, the host
//!   ops and their policy; [`webhooks`] and [`app_function_executor`] run them
//!   from a provider's call or the task queue.
//! - [`custom_apps_publish`] (+ `_form`, `_oidc`, `_audit`, `_refusal`),
//!   [`custom_apps_promote`], [`custom_apps_sandboxes`], [`custom_apps_secrets`],
//!   [`custom_apps_migrations`], [`custom_apps_nonproduction`],
//!   [`custom_apps_env_schedules`], [`custom_apps_tail`]: the lifecycle.
//! - [`admin`]: the staff console, `/api/customer-apps`, behind
//!   [`admin::apps::guarded`].
//! - [`custom_apps_activity`], [`custom_apps_shell_context`],
//!   [`custom_apps_threads`], [`custom_apps_availability`],
//!   [`custom_apps_debug`], [`workspace_custom_apps`], [`me_apps`]: the reads
//!   a bundle and the launcher make.
//!
//! What this crate takes from lower crates is in `oxy-custom-apps` (the serve
//! resolver, the gate, the build store and its byte cache, the manifest, storage,
//! the usage queue) and `oxy-app-core` (the agent-token body, the function seam
//! traits). From `oxy-app` it takes `oxy_app::surface` and nothing else (rule S4,
//! `tests/routing/surface_contract.rs`).
//!
//! Mounted by the `oxy-server` composition root through these seams:
//!
//! - [`routes`] on `SurfaceSeams::api` (authenticated, cloud only), with
//!   [`route_roles`];
//! - [`workspace_routes`] on `SurfaceSeams::workspace`, with
//!   [`workspace_route_roles`];
//! - [`public_routes`] on `SurfaceSeams::public`, merged beside oxy-app's
//!   `build_public_routes` with **no** auth layer. Each handler authenticates
//!   inline (a session cookie, a bearer token, or an HMAC), and does so before it
//!   looks the app up, so an anonymous caller cannot tell whether an app exists;
//! - [`serve::serve_dispatch`] on `SurfaceSeams::outer`, the top-level
//!   `/customer-apps/{*path}` route (its layers are built by the composition
//!   root), with [`serve::serve_dispatch_roles`] as `SurfaceSeams::outer_decls`;
//! - the task executors, background jobs, [`function_runtime`], the `oxy migrate`
//!   report and the demo seed's half, which `oxy-server`'s `custom_apps_seams`
//!   builds from the items below.

// The serve route's chained cache, store and function futures exceed the default
// type-layout query depth of 128, as the same code did in `oxy-app`.
#![recursion_limit = "256"]

pub mod admin;
mod app_function_agent;
pub mod app_function_executor;
pub mod app_lifecycle;
pub mod beacon;
pub mod client;
pub mod custom_app_provenance;
pub mod custom_app_template;
pub mod custom_apps_activity;
pub mod custom_apps_agent_built;
pub mod custom_apps_agent_refusal;
pub mod custom_apps_availability;
pub mod custom_apps_build_history;
pub mod custom_apps_debug;
pub mod custom_apps_env_schedules;
pub mod custom_apps_functions;
pub mod custom_apps_migrations;
pub mod custom_apps_nonproduction;
pub mod custom_apps_open_builds;
/// `oxyc promote` / `oxyc rollback`, and the one function that moves
/// production's pointer for every writer.
pub mod custom_apps_promote;
pub mod custom_apps_publish;
/// The audit row of a bundle publish, and what one says of a publish token.
pub(crate) mod custom_apps_publish_audit;
pub mod custom_apps_publish_form;
pub mod custom_apps_publish_oidc;
pub mod custom_apps_publish_refusal;
pub mod custom_apps_sandboxes;
pub mod custom_apps_scaffold;
pub mod custom_apps_secrets;
pub mod custom_apps_shell_context;
pub mod custom_apps_staging_held;
pub mod custom_apps_tail;
pub mod custom_apps_threads;
/// The task that copies a workspace's usage history into its Airhouse, and the
/// sweep that queues it.
pub mod custom_apps_usage_backfill;
pub mod custom_apps_validate;
pub mod custom_domains;
pub mod function_runtime;
pub mod health;
pub mod home_widgets;
pub mod html_cache;
mod invocation_placement;
pub mod logs;
pub mod me_apps;
pub mod seed;
pub mod seed_apps;
pub mod seed_fixture;
pub mod seed_storage;
pub mod serve;
pub mod sourcemap;
pub mod webhooks;
pub mod workspace_custom_apps;

// ── the names the moved modules reach their neighbours by ─────────────────────
//
// The modules above came out of `oxy-app`'s `server::api`, where every
// `custom_apps_*` module and the lowered ones' aliases sat side by side in
// `api/mod.rs`, and a module reached a sibling as `super::custom_apps_x`. They
// sit at this crate's root now, so the same paths resolve here, to the same
// code: the aliases below are `api/mod.rs`'s, pointing at the crates the
// modules were lowered into (`oxy-custom-apps`, `oxy-app-core`).
pub use oxy_app_core::agent_body as custom_apps_agent_body;
pub use oxy_custom_apps::asset_manifest as custom_apps_asset_manifest;
pub use oxy_custom_apps::build_state as custom_apps_build_state;
pub use oxy_custom_apps::build_store as custom_apps_build_store;
pub use oxy_custom_apps::bundle_cache as custom_apps_bundle_cache;
pub use oxy_custom_apps::cache as custom_apps_cache;
pub use oxy_custom_apps::call_trace as custom_apps_call_trace;
pub use oxy_custom_apps::call_trace_store as custom_apps_call_trace_store;
pub use oxy_custom_apps::env_resolve as custom_apps_env_resolve;
pub use oxy_custom_apps::environments as custom_apps_environments;
pub use oxy_custom_apps::manifest as custom_apps_manifest;
pub use oxy_custom_apps::non_production as custom_apps_non_production;
pub use oxy_custom_apps::precompress as custom_apps_precompress;
pub use oxy_custom_apps::publish_authz as custom_apps_publish_authz;
pub use oxy_custom_apps::sandbox_instance as custom_apps_sandbox_instance;
pub use oxy_custom_apps::source as custom_apps_source;
pub use oxy_custom_apps::storage as custom_apps_storage;
pub use oxy_custom_apps::sync as custom_apps_sync;
pub use oxy_custom_apps::telemetry as custom_apps_telemetry;
pub use oxy_custom_apps::tracking as custom_apps_tracking;
pub use oxy_custom_apps::updates as custom_apps_updates;
pub use oxy_custom_apps::usage_airhouse as custom_apps_usage_airhouse;
pub use oxy_custom_apps::usage_queue as custom_apps_usage_queue;
pub use oxy_custom_apps::usage_reads as custom_apps_usage_reads;

/// A sandbox agent token on the custom-app serving paths: the admission rules
/// (`oxy_custom_apps::agent`) and the usage count that wraps `/fn` and `/logs`.
pub mod custom_apps_agent {
    pub use oxy_app_core::usage_probe::{UsageProbe, seen};
    pub use oxy_custom_apps::agent::*;
}

/// Who may open an app, and the request-facing flow every handler calls.
pub mod custom_apps_auth {
    pub use oxy_custom_apps::auth::*;
    pub use oxy_custom_apps::authenticate::*;
}

/// The data-plane gate chain, answered as a response: `oxy_custom_apps::gate`
/// holds the steps and the refusals; this maps a refusal onto the JSON envelope
/// the SDK reads (`oxy_app_core::gate_response`).
pub mod custom_apps_gates {
    use axum::http::HeaderMap;
    use axum::response::Response;
    use uuid::Uuid;

    pub use oxy_custom_apps::gate::{CustomAppContext, frontline_worker_with_app_grant};

    /// `oxy_custom_apps::gate::check_custom_app_gates`, with a refusal turned
    /// into its response.
    pub async fn check_custom_app_gates(
        headers: &HeaderMap,
        project_id: Uuid,
    ) -> Result<CustomAppContext, Response> {
        oxy_custom_apps::gate::check_custom_app_gates(headers, project_id)
            .await
            .map_err(|e| oxy_app_core::gate_response::refusal(e.status(), e.message(), e.code()))
    }
}

/// A staging build's semantic pin and the request that names one.
pub mod custom_apps_staging_pin {
    pub use oxy_app_core::staging_pin_scope::{
        cache_partition, current_staging_pin, with_staging_pin,
    };
    pub use oxy_custom_apps::staging_pin::request;
    pub use oxy_custom_apps::staging_pin::request::{APP_HEADER, staging_pin_for_data_request};
    pub use oxy_custom_apps::staging_pin::{
        PinRefusal, SemanticPinNotice, check_pinnable, pin_drift, pinned_revision_for,
        validate_pin_for_publish,
    };
}

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{delete, get, post};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

const FLEET: RouteRole = RouteRole::FleetOk;

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: FLEET,
    }
}

/// The authenticated routes: the staff console at `/customer-apps`, the caller's
/// own app list and the launcher.
///
/// Merged into the protected tree before the auth stack is applied
/// (`SurfaceSeams::api`, cloud mode only — local mode never mounts the global
/// tree these were part of), so they inherit it exactly as they did in `oxy-app`.
///
/// The custom-app admin surface, and the ONE registration of the admin app
/// handlers: [`admin::apps::router`] (create / list / get / update / delete,
/// publish, functions, read-back, issues, access, builds, rollback, batch,
/// templates, storage) plus the sibling modules' routes below, all behind the
/// single [`admin::apps::guarded`] stack. Gated by `PlatformApps`, not by
/// OXY_OWNER, so an App Operator can run the lifecycle without org or billing
/// access.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/apps/mine", get(admin::apps::handlers::list_my_apps))
        // The mobile home screen.
        .route("/me/apps", get(me_apps::list_openable_apps))
        // Each app's live cards on the Oxygen home (`home_widgets`).
        .route("/home/widgets", get(home_widgets::list))
        .route(
            "/home/widgets/{app_id}/{widget}/actions/{action}",
            post(home_widgets::press),
        )
        .nest(
            "/customer-apps",
            admin::apps::guarded(
                admin::apps::router()
                    // Activity (usage tracking) — see `custom_apps_activity`.
                    // Reads usage through `oxy_custom_apps::usage_reads` to
                    // power the AppDetail "Activity" tab.
                    .route(
                        "/{id}/activity/summary",
                        get(custom_apps_activity::get_summary),
                    )
                    .route(
                        "/{id}/activity/visitors",
                        get(custom_apps_activity::get_visitors),
                    )
                    .route(
                        "/{id}/activity/events",
                        get(custom_apps_activity::get_events),
                    )
                    // What the caller's own staging requests held, newest first —
                    // the console's staging banner. 404 unless oxy-authz lets the
                    // caller open the app's staging. See `custom_apps_staging_held`.
                    .route(
                        "/{id}/staging/held",
                        get(custom_apps_staging_held::list_held),
                    )
                    // Org / project browser: every workspace that granted Oxy
                    // access, flattened with its org + grant metadata. See
                    // `admin::oxy_access`.
                    .route("/oxy-access", get(admin::oxy_access::list_grants))
                    // App-scoped secrets (`apps/<app_id>/<KEY>` — the namespace
                    // `ctx.env` reads). The list is reconciled against what the
                    // active build DECLARES, so a fresh deploy says which keys are
                    // still missing instead of failing at the first invocation.
                    // Creation lives only here and on the tenant twin: the project
                    // secrets API rejects `/` in a name, which is why an app secret
                    // had no write path at all. See `custom_apps_secrets`.
                    .route(
                        "/{id}/secrets",
                        get(custom_apps_secrets::admin_list).post(custom_apps_secrets::admin_set),
                    )
                    .route(
                        "/{id}/secrets/{key}",
                        delete(custom_apps_secrets::admin_delete),
                    )
                    .route(
                        "/{id}/secrets/{key}/value",
                        get(custom_apps_secrets::admin_reveal),
                    )
                    // Sandbox environments (`dev-<handle>`): create, list, show and
                    // delete — see `custom_apps_sandboxes` and
                    // `internal-docs/custom-app-sandboxes.md` §5.1. Postgres only (the
                    // teardown is a queued task), so FleetOk like the rest of the nest.
                    // Registered above the four layers so all of them cover it;
                    // each handler then also requires `Action::AppNonProduction` over
                    // the app and refuses a publish token. The app id is named `id`
                    // because `enforce_app_scope` reads it by that name.
                    .route(
                        "/{id}/environments",
                        get(custom_apps_sandboxes::handlers::list)
                            .post(custom_apps_sandboxes::handlers::create),
                    )
                    .route(
                        "/{id}/environments/{name}",
                        get(custom_apps_sandboxes::handlers::show)
                            .delete(custom_apps_sandboxes::handlers::delete),
                    )
                    // Promote and rollback (`custom_apps_promote`; environments
                    // design §5.5a): a sandbox's build to staging, staging's to
                    // production, and either of those two back. Postgres, the
                    // object store and the apps' migration connections — no working
                    // copy, so FleetOk with the nest; production's migrations run
                    // inside the request, as a promoting publish's do. Registered
                    // above the four layers so all of them cover it; each handler
                    // also refuses a sandbox agent token and a publish token.
                    // `{name}` is the environment that moves — the contract's
                    // `{to}` for a promote.
                    .route(
                        "/{id}/environments/{name}/promote",
                        post(custom_apps_promote::handlers::promote),
                    )
                    .route(
                        "/{id}/environments/{name}/rollback",
                        post(custom_apps_promote::handlers::rollback),
                    )
                    // Live tail of one environment's function invocations (SSE) —
                    // see `custom_apps_tail`. FleetOk with the nest: producers
                    // `NOTIFY` and every replica `LISTEN`s, so any replica holds
                    // the stream whichever process runs the function. Under the
                    // four layers, like the routes above; the handler then asks
                    // production for an app admin and every other environment for
                    // `Action::AppNonProduction`.
                    .route(
                        "/{id}/environments/{name}/tail",
                        get(custom_apps_tail::handler::tail),
                    )
                    // A non-production environment's schedules switch (off by
                    // default) and its rows: read it back, or flip it and
                    // reconcile the rows. Postgres only — FleetOk with the nest.
                    // See `custom_apps_env_schedules`.
                    .route(
                        "/{id}/environments/{name}/schedules",
                        get(custom_apps_env_schedules::handlers::show)
                            .put(custom_apps_env_schedules::handlers::set),
                    )
                    // Trusted-publishing config: register / list / remove the GitHub
                    // workflows allowed to OIDC-publish this app. See
                    // `custom_apps_publish_oidc`.
                    .route(
                        "/{id}/publishers",
                        get(custom_apps_publish_oidc::list_publishers)
                            .post(custom_apps_publish_oidc::register_publisher),
                    )
                    .route(
                        "/{id}/publishers/{publisher_id}",
                        delete(custom_apps_publish_oidc::delete_publisher),
                    ),
            )
            // Everything ABOVE is the interactive customer-apps admin surface; the
            // stack it sits behind (`admin::apps::guarded`) covers the routes
            // registered before it and NOT `/publish` below.
            // One-way publish entry point: CI (or local `oxyc publish`) uploads
            // a built bundle tarball. Registered AFTER both layers above, so
            // it is neither app-admin-gated nor blocked-while-acting:
            //   * NOT app-admin-gated — authorization is decided INSIDE
            //     `publish()` by the three gates (org officer, or a partner
            //     with manage_apps + assignment + the client's consent, or
            //     staff-unless-locked). Gating the route to app_admins would
            //     have made partner publish impossible.
            //   * NOT blocked-while-acting — it is also the CI endpoint reached
            //     by a publish token; blocking it would 403 a CI job merely
            //     because the token's minter has a browser assume-session open.
            // It remains behind the outer auth + `app_publish_token_scope`
            // layers, so it is always authenticated. Raised body limit —
            // bundles are a few MB, over axum's 2 MB default.
            .route(
                "/publish",
                post(custom_apps_publish::publish_handler)
                    .layer(DefaultBodyLimit::max(64 * 1024 * 1024)),
            ),
        )
}

/// What [`routes`] declares about pod placement: all FleetOk (registry rows plus
/// S3 bundles, a queued teardown, Postgres reads — no working copy). Relative to
/// the tree the seam merges into; the router adds the `/api` prefix.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    const ROLES: &[RouteRoleDecl] = &[
        fleet("/apps/mine"),
        fleet("/me/apps"),
        fleet("/home/widgets"),
        fleet("/home/widgets/{app_id}/{widget}/actions/{action}"),
        fleet("/customer-apps/{*rest}"),
        fleet("/customer-apps"),
    ];
    ROLES
}

/// The workspace-scoped routes: the workspace's own app list, and creating an app
/// secret from the tenant side (`apps/<app_id>/<KEY>`).
///
/// POST only for the secret: list / reveal / rotate / delete already work on the
/// project-secrets routes, which address a row by id and so never hit the name
/// validator that rejects the `/`. Same handler as the staff
/// `/customer-apps/{id}/secrets` mount, with a `WorkspaceAdmin` guard and a check
/// that the app belongs to the workspace in the path. Postgres only, so FleetOk.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .route("/custom-apps", get(workspace_custom_apps::list_custom_apps))
        .route(
            "/custom-apps/{app_id}/secrets",
            post(custom_apps_secrets::workspace_set),
        )
}

/// What [`workspace_routes`] declares, relative to the `/{workspace_id}` nest.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    const ROLES: &[RouteRoleDecl] = &[
        fleet("/custom-apps"),
        fleet("/custom-apps/{app_id}/secrets"),
    ];
    ROLES
}

/// The routes that authenticate inline (or by an HMAC) and so sit outside the
/// auth stack.
///
/// External liveness for ONE published custom app is in the public router
/// because it authenticates inline (session cookie or bearer token) like its
/// neighbours — and the token is mandatory: auth runs before the app lookup so
/// an unauthenticated caller gets 401 whether or not the app exists, which is
/// what keeps this from becoming an app-enumeration oracle. Two forms:
/// slug-explicit (pollable from the admin host, one monitor for every app) and
/// Host-resolved (`/customer-apps/health` on an `<org>--<slug>.customer-apps.<zone>`
/// subdomain, where `/api/*` is the only prefix the subdomain rewrite passes
/// through). NOT a k8s probe — see the [`health`] module docs.
///
/// Per-app debuggability — persisted Oxy Function output, and browser errors
/// with source maps applied server-side — has the same inline AUTHENTICATION as
/// its neighbours, but a narrower AUTHORIZATION: both call `require_app_admin`,
/// because function log output is what the author printed while debugging, not
/// the app's own data that every org member may already see.
///
/// The webhooks are unauthenticated by design — the sender is proven by an HMAC
/// the PLATFORM verifies against a secret the app's manifest declares, before
/// any app code runs. A function that declares no `webhook:` block is a 404, so
/// this cannot be walked to enumerate an app's functions. FleetOk: a provider
/// retrying against whichever instance it reaches has no idea which one owns a
/// workspace, and a webhook that only works on the singleton silently drops
/// events.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/customer-apps/{org_slug}/{app_slug}/health",
            get(health::get_health),
        )
        .route("/customer-apps/health", get(health::get_health_for_host))
        .route(
            "/customer-apps/{org_slug}/{app_slug}/logs",
            get(logs::get_logs),
        )
        .route(
            "/customer-apps/{org_slug}/{app_slug}/errors",
            get(logs::get_errors),
        )
        // Trusted-publishing OIDC exchange — unauthenticated by construction: the
        // GitHub Actions OIDC JWT in the Authorization header IS the credential.
        // Returns a short-lived, app-scoped publish token.
        // Custom domains: the sign-in handoff (authorize on the product host,
        // session on the custom domain — each checks which host it is on) and
        // the TLS edge's "may I issue for this name?". Each is a browser
        // navigation or an edge probe that authenticates inline or not at all;
        // all Postgres reads behind the resolver cache, so FleetOk.
        .route(
            "/custom-domains/authorize",
            get(custom_domains::handoff::authorize),
        )
        .route(
            "/custom-domains/session",
            get(custom_domains::handoff::session),
        )
        .route(
            "/custom-domains/tls-ask",
            get(custom_domains::handoff::tls_ask),
        )
        .route(
            "/customer-apps/publish/oidc-exchange",
            post(custom_apps_publish_oidc::oidc_exchange_handler),
        )
        .route(
            "/webhooks/apps/{org_slug}/{app_slug}/{function_name}",
            post(webhooks::app_function::app_function_webhook),
        )
        // The same, for a non-production environment (`staging`, `dev-<handle>`),
        // named in the path so a provider that drops a query string cannot land
        // on production — which this route never reaches. Verified with that
        // environment's own secret and no fallback; every unknown, deleting,
        // expired or build-less environment is the same 404.
        .route(
            "/webhooks/apps-env/{environment}/{org_slug}/{app_slug}/{function_name}",
            post(webhooks::app_function_env::app_function_env_webhook),
        )
        // Public build-config endpoint — no auth required. CI reads this at
        // build time so no per-app env vars need to live in customer-apps repo.
        .route(
            "/apps/{org_slug}/{app_slug}/build-config",
            get(admin::apps::handlers::get_build_config),
        )
        // Public: resolve a workspace's org slug so `oxyc publish --project <id>`
        // can bake the /customer-apps/<org>/<app>/ base path without a hardcoded
        // orgSlug (same rationale as build-config — ids/slugs aren't secrets).
        .route(
            "/org-for-project/{project_id}",
            get(admin::apps::handlers::get_org_for_project),
        )
        // Diagnostic snapshot for admins — what the server sees about this app +
        // its manifest. Same cookie-auth + org-membership gate; human inspection
        // only, shape not guaranteed stable.
        .route(
            "/customer-apps/{org_slug}/{app_slug}/debug",
            get(custom_apps_debug::get_debug),
        )
        // The SERVING answer, as opposed to `/health`'s deployment-integrity
        // ladder: the success ratio of real traffic over several windows, plus a
        // multi-window error-budget burn verdict. A ClickHouse read behind the
        // same inline auth gate as its neighbours, so FleetOk — an availability
        // endpoint pinned to the singleton would go dark exactly when that
        // singleton is the thing in trouble.
        .route(
            "/customer-apps/{org_slug}/{app_slug}/availability",
            get(custom_apps_availability::get_availability),
        )
        // Shell bootstrap for `@oxy-hq/sdk/shell` chrome inside bundles:
        // workspace/org identity + published-apps list + host-aware product links.
        // Pure Postgres reads behind the data-plane gate chain.
        .route(
            "/projects/{project_id}/shell-context",
            get(custom_apps_shell_context::get_shell_context),
        )
        // Bundle chat history for the SDK Ask dock. `/threads` lists the viewer's
        // threads; `/threads/{id}` rebuilds a transcript for restore.
        .route(
            "/projects/{project_id}/threads",
            get(custom_apps_threads::list_threads),
        )
        .route(
            "/projects/{project_id}/threads/{thread_id}",
            get(custom_apps_threads::get_thread_transcript),
        )
        // Bundle-SDK custom event ingest — `useTrackEvent("name", {...})`. Shares
        // the data-plane gate chain with `/query`. See `custom_apps_activity::post_event`.
        .route(
            "/customer-apps/{project_id}/events",
            post(custom_apps_activity::post_event),
        )
}

/// Every route [`public_routes`] mounts, and which pod may serve it. All FleetOk:
/// health reads the build store and Postgres, logs are ClickHouse reads plus
/// build-store maps, and the rest are Postgres and the secret store, which every
/// replica has — declared explicitly because the type gate
/// `RoleRouter::route_fleet` used to provide does not reach across the crate
/// line. Paths are absolute: the public seam merges this crate at the `/api` root.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    const ROLES: &[RouteRoleDecl] = &[
        fleet("/customer-apps/{org_slug}/{app_slug}/health"),
        fleet("/customer-apps/health"),
        fleet("/customer-apps/{org_slug}/{app_slug}/logs"),
        fleet("/customer-apps/{org_slug}/{app_slug}/errors"),
        fleet("/custom-domains/authorize"),
        fleet("/custom-domains/session"),
        fleet("/custom-domains/tls-ask"),
        fleet("/customer-apps/publish/oidc-exchange"),
        fleet("/webhooks/apps/{org_slug}/{app_slug}/{function_name}"),
        fleet("/webhooks/apps-env/{environment}/{org_slug}/{app_slug}/{function_name}"),
        fleet("/apps/{org_slug}/{app_slug}/build-config"),
        fleet("/org-for-project/{project_id}"),
        fleet("/customer-apps/{org_slug}/{app_slug}/debug"),
        fleet("/customer-apps/{org_slug}/{app_slug}/availability"),
        fleet("/projects/{project_id}/shell-context"),
        fleet("/projects/{project_id}/threads"),
        fleet("/projects/{project_id}/threads/{thread_id}"),
        fleet("/customer-apps/{project_id}/events"),
    ];
    ROLES
}

// Last in the file on purpose: `oxy-route-catalog`'s build script reads a file only up to
// its first `#[cfg(test)]`, and the route builders above must be in what it reads.
#[cfg(test)]
pub(crate) use oxy_app_core::agent_fixture as custom_apps_agent_fixture;
