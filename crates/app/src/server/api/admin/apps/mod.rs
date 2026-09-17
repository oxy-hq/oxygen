//! Customer-apps registry — admin endpoints and types.
//!
//! Mounted at `/api/admin/apps` via `router()`. Gated by oxy_owner_guard
//! at the router layer (in `router/global.rs`). Customer apps are routed and
//! served entirely inside oxy (see `custom_apps_serve`); no external
//! routing infra (CloudFront/Route53) sits in the data path.
//!
//! Build-time config (project_id + branch) is served publicly via
//! `GET /apps/{id}/build-config` so that CI can fetch it without any
//! per-app repo variables.

pub mod access;
mod dto;
pub mod fleet_health;
pub mod functions;
pub mod handlers;
mod ops;
/// Slug validation, re-exported so the (non-admin) `/publish` route can reject a
/// slug before it becomes a schema name / path — see `custom_apps_publish`.
pub(crate) use ops::is_valid_slug;
pub mod storage;
pub mod templates;

use axum::Router;
use axum::routing::{delete, get, patch, post};

use crate::server::router::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/apps", post(handlers::create_app))
        .route("/apps", get(handlers::list_apps))
        // Fleet health — every published app and whether it is working.
        // Declared BEFORE `/apps/{id}` would ever match it; axum routes
        // literals over captures, but the ordering documents the intent.
        .route("/apps/health", get(fleet_health::get_fleet_health))
        .route("/apps/{id}", get(handlers::get_app))
        .route("/apps/{id}", patch(handlers::update_app))
        .route("/apps/{id}", delete(handlers::delete_app))
        .route(
            "/apps/{id}/publish",
            post(handlers::publish_app).delete(handlers::unpublish_app),
        )
        // Oxy Functions management/debug surface for the AppDetail Functions
        // section: list the app's functions + config, one function's recent
        // invocation history, and a job run's status + logs. The write —
        // triggering a run — is the `runs` route below.
        // See internal-docs/customer-apps-functions.md.
        .route("/apps/{id}/functions", get(functions::list_functions))
        .route(
            "/apps/{id}/functions/{name}/invocations",
            get(functions::list_invocations),
        )
        .route(
            "/apps/{id}/function-runs/{run_id}",
            get(functions::get_function_run),
        )
        // Manually trigger a one-off background run of one of the app's Oxy
        // Functions as a job (the "run now" that isn't tied to a cron schedule).
        .route(
            "/apps/{id}/functions/{name}/runs",
            post(handlers::run_function_job),
        )
        // Who may open the app. Staff-gated twin of the org's own
        // `/organizations/{id}/apps/{id}/access` — the org route needs an
        // assume-role session, which `block_admin_while_acting` makes mutually
        // exclusive with standing in this console. Same service, different gate.
        .route(
            "/apps/{id}/access",
            get(access::get_app_access).put(access::set_app_access),
        )
        .route("/apps/{id}/teams", get(access::list_app_org_teams))
        .route("/apps/{id}/members", get(access::list_app_org_members))
}
