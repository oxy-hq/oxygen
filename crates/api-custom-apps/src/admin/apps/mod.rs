//! Customer-apps registry — admin endpoints and types.
//!
//! One registration, one guard stack. [`router`] is every admin app handler,
//! mounted at `/api/customer-apps` by `router/global.rs` behind [`guarded`].
//! There is no other mount: `/api/admin/apps` was removed once `oxyc` called
//! `/api/customer-apps`. Customer apps are routed and
//! served entirely inside oxy (see `custom_apps_serve`); no external
//! routing infra (CloudFront/Route53) sits in the data path.
//!
//! Build-time config (project_id + branch) is served publicly via
//! `GET /apps/{id}/build-config` so that CI can fetch it without any
//! per-app repo variables.

pub mod access;
/// A sandbox agent token on the verify and read-back routes: an own sandbox
/// is required, everything else is a not-found.
mod agent_scope;
mod dto;
/// The `?environment=` parameter and who may name a non-production one.
pub mod environment_scope;
pub mod fleet_health;
/// Failed production invocations across every app in scope, in a time window.
pub mod function_failures;
mod function_run;
pub mod functions;
pub mod handlers;
/// The held-write read-back of one non-production invocation.
pub mod held_writes;
/// The call trace of one invocation: the console's waterfall.
pub mod invocation_calls;
/// The fences the per-invocation read-back routes share.
mod invocation_read;
/// The invocation query, its DTO, and the app-wide listing.
pub mod invocations;
/// An app's failures grouped per `(function, fingerprint)` — the pager's key.
pub mod issues;
mod ops;
/// Production's releases over a window, each beside the one before it, and
/// the three environments' pointer moves.
pub mod releases;
/// Slug validation, re-exported so the (non-admin) `/publish` route can reject a
/// slug before it becomes a schema name / path — see `custom_apps_publish`.
pub(crate) use ops::is_valid_slug;
/// The list rows' `availability` field: the per-app availability reading, in one read.
pub mod list_availability;
/// Run now `as=schedule`: a scheduled function run once as its schedule would.
pub mod run_as_schedule;
/// The audit row of "run now", for every credential.
mod run_audit;
pub mod storage;
pub mod templates;
/// The console's "Test notifications": one sent as the app, to the caller alone.
pub mod test_notification;
/// One answer to "is this app OK?" on the list rows and launcher summaries.
pub mod verdict;

use axum::Router;
use axum::middleware;
use axum::routing::{get, post};

use oxy_app::surface::assume::block_admin_while_acting;
use oxy_app::surface::guards::oxy_owner_or_app_admin_guard_middleware;
use oxy_app::surface::guards::{app_scope_guard, platform_cap_guard};
use oxy_app_core::AppState;
use oxy_server_authz::Action;

/// The one guard stack of the custom-app admin surface, innermost first:
///
/// 1. `block_admin_while_acting` — refused while a staff operator is acting as
///    a tenant; you cannot wield staff powers and wear a customer's identity.
/// 2. `app_scope_guard::enforce_app_scope` — which apps this grant may touch,
///    read from the matched `{id}`.
/// 3. `Action::PlatformApps` — the capability the lifecycle IS (`Cap::ManageApps`),
///    so an App Operator who is not a Global Owner reaches it.
/// 4. `oxy_owner_or_app_admin_guard_middleware` — owner OR app admin, not
///    admin-only: a Global Owner who is not also in `app_admins` must not be
///    locked out of a surface the junior role runs.
///
/// Applies to the routes registered BEFORE it (axum's `.layer` rule): a route that
/// must stay outside the stack (`/customer-apps/publish`, which decides its own
/// authorization inside `publish()`) is added after.
pub fn guarded<S>(routes: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    routes
        .layer(middleware::from_fn(block_admin_while_acting))
        .layer(middleware::from_fn(app_scope_guard::enforce_app_scope))
        .layer(middleware::from_fn(platform_cap_guard::require(
            Action::PlatformApps,
        )))
        .layer(middleware::from_fn(oxy_owner_or_app_admin_guard_middleware))
}

/// Every admin app handler, paths relative to `/api/customer-apps`.
pub(crate) fn router() -> Router<AppState> {
    core_routes().merge(console_routes())
}

/// The registry, publish, functions, read-back, access, builds and storage routes.
fn core_routes() -> Router<AppState> {
    Router::new()
        // The list wraps `handlers::list_apps` to add each row's `verdict`.
        .route(
            "/",
            post(handlers::create_app).get(verdict::list_apps_with_verdict),
        )
        // Fleet health — every published app and whether it is working. On the
        // app-admin surface because an App Operator ships these apps and "which of
        // mine are broken" is their question; scope still filters the rows.
        //
        // `fleet-health`, NOT `/health`: `public.rs` owns `/customer-apps/health`
        // for ONE published app's external liveness, resolved from the `Host` on a
        // custom-app subdomain, and outside monitors poll it. That tree is merged
        // into this one in `entry::api_router`, so the two names must differ or the
        // whole router panics at construction (#3207 shipped exactly that).
        .route("/fleet-health", get(fleet_health::get_fleet_health))
        // Failed production invocations across every app in scope, by time:
        // what ties one platform bug to every app it broke.
        .route(
            "/function-failures",
            get(function_failures::list_function_failures),
        )
        .route(
            "/{id}",
            get(handlers::get_app)
                .patch(handlers::update_app)
                .delete(handlers::delete_app),
        )
        .route(
            "/{id}/publish",
            post(handlers::publish_app).delete(handlers::unpublish_app),
        )
        // Oxy Functions management/debug surface for the AppDetail Functions
        // section: list the app's functions + config, one function's recent
        // invocation history, and a job run's status + logs. The write —
        // triggering a run — is the `runs` route below.
        // See internal-docs/customer-apps-functions.md.
        .route("/{id}/functions", get(functions::list_functions))
        .route(
            "/{id}/functions/{name}/invocations",
            get(functions::list_invocations),
        )
        .route(
            "/{id}/function-runs/{run_id}",
            get(functions::get_function_run),
        )
        // Read-back: every function's invocations for the app, filtered by
        // environment and build, and what one non-production invocation's policy
        // held instead of performing. DB reads — FleetOk.
        .route("/{id}/invocations", get(invocations::list_app_invocations))
        .route(
            "/{id}/invocations/{invocation_id}/held",
            get(held_writes::get_held_writes),
        )
        // Every ctx.* call one invocation made, as timed records (the
        // console's waterfall). A DB read, like its neighbours.
        .route(
            "/{id}/invocations/{invocation_id}/calls",
            get(invocation_calls::get_invocation_calls),
        )
        // The same failed invocations, one row per distinct failure. Derived
        // on every read, production only — a DB read like its neighbours.
        .route("/{id}/issues", get(issues::list_app_issues))
        // Production's releases over `?days=`, each with its failure rate, route
        // p95, Web Vitals per screen and migrations beside the release before it,
        // and staging's and sandboxes' moves for a caller with non-production
        // reach. Derived on every read from Postgres — FleetOk.
        .route("/{id}/releases", get(releases::list_app_releases))
        // Manually trigger a one-off background run of one of the app's Oxy
        // Functions as a job (the "run now" that isn't tied to a cron schedule).
        .route(
            "/{id}/functions/{name}/runs",
            post(handlers::run_function_job),
        )
        // Who may open the app. Staff-gated twin of the org's own
        // `/organizations/{id}/apps/{id}/access` — the org route needs an
        // assume-role session, which `block_admin_while_acting` makes mutually
        // exclusive with standing in this console. Same service, different gate.
        // These three list people, so a publish token's read grant excludes them
        // (`app_publish_token_scope::is_allowed`).
        .route(
            "/{id}/access",
            get(access::get_app_access).put(access::set_app_access),
        )
        .route("/{id}/teams", get(access::list_app_org_teams))
        // Customer-owned hostnames the app's production is also served at: add
        // one, prove it by DNS, remove it. Postgres plus one DNS-over-HTTPS read
        // per verify — FleetOk with the nest.
        .route(
            "/{id}/domains",
            get(crate::custom_domains::admin::list_domains)
                .post(crate::custom_domains::admin::add_domain),
        )
        .route(
            "/{id}/domains/{domain_id}",
            axum::routing::delete(crate::custom_domains::admin::remove_domain),
        )
        .route(
            "/{id}/domains/{domain_id}/verify",
            post(crate::custom_domains::admin::verify_domain),
        )
        .route("/{id}/members", get(access::list_app_org_members))
        // "Test notifications": the kinds the live build declares, and one sent
        // as the app to the caller alone, through `ctx.notify`'s send path. DB
        // reads and a queued delivery — FleetOk, like the rest of this tree.
        .route(
            "/{id}/notification-kinds",
            get(test_notification::list_kinds),
        )
        .route(
            "/{id}/notification-test",
            post(test_notification::send_test_notification),
        )
}

/// What only `/api/customer-apps` serves: the console's lifecycle, batch and
/// storage routes.
fn console_routes() -> Router<AppState> {
    Router::new()
        // New-pipeline build lifecycle: list versioned builds and roll the
        // published channel back to any retained one. Pointer moves only —
        // bytes already live in S3.
        .route("/{id}/builds", get(handlers::list_builds))
        .route("/{id}/rollback", post(handlers::rollback_app))
        // Batch mutations for the admin apps table: publish / unpublish / delete
        // many apps in one request. POST even for delete, since the id set travels
        // in the body. Each is best-effort per-id (see `BatchResponse`); `batch`
        // is a static segment so it never collides with `/{id}`.
        .route("/batch/publish", post(handlers::batch_publish_apps))
        .route(
            "/batch/promote-latest",
            post(handlers::batch_promote_latest_apps),
        )
        .route("/batch/unpublish", post(handlers::batch_unpublish_apps))
        .route("/batch/delete", post(handlers::batch_delete_apps))
        // Template gallery for the Create-new dialog. No screenshot route yet —
        // re-add when the first PNG ships (see templates.rs module docstring).
        .route("/templates", get(templates::list_templates))
        // ── Custom-app storage (see the asset-lifecycle design doc) ──
        // Fleet view: every measured app ranked by size / 7d growth / untagged
        // bytes. Reads the `app_storage_usage` rollup, never S3 — ranking the fleet
        // cannot mean walking every silo per request.
        .route("/storage", get(storage::fleet))
        // Force a re-measure. Without this the fleet view's staleness is visible
        // but unactionable.
        .route("/storage/sweep", post(storage::sweep_now))
        // Daily totals for the usage-over-time chart. Fleet-wide by default;
        // `?appId=` narrows it to one app.
        .route("/storage/history", get(storage::history))
        // Month-to-date GB-month for one org (time-weighted).
        .route("/storage/meter/{org_id}", get(storage::org_meter))
        // Per-app browser: reads S3 live via `list()`, because the rollup holds no
        // per-object rows and an operator investigating now needs current truth,
        // not a number a sweep old.
        .route("/{id}/storage/objects", get(storage::browse))
        .route("/{id}/storage/delete", post(storage::delete_objects))
}
