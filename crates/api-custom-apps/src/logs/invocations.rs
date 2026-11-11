//! `GET /api/customer-apps/{org}/{app}/invocations` — an app's recorded
//! function invocations, addressed by slug and gated exactly as `/logs` is.
//!
//! The staff console already lists these at `/customer-apps/{id}/invocations`
//! (`admin::apps::invocations`), behind the console's stack: `PlatformApps`,
//! the grant's app scope, and no active assume-role session. That stack turns
//! away the two callers who most need the read while operating an app: staff
//! inside an assume session, and the tenant's own app admins. Both already
//! read the app's function OUTPUT on `/logs`; this is the same authority over
//! the invocations that output came from.
//!
//! ## The gate is the logs route's, not a second one
//!
//! `authenticate_and_authorize` (inline auth, the resolver's 401/404/403),
//! then `environment::admitted`: `require_app_admin` (oxy-authz
//! `Ring::AppAdmin`) for every read, and `may_open_non_production`
//! (`Action::AppNonProduction`) for a non-production environment. Naming no
//! environment reads every environment for a caller with that reach and
//! production's for one without, as the console's listing does; the response
//! says which (`environments`: `"all"` or the one name). See [`read_scope`].
//!
//! ## Metadata only
//!
//! Each row is `InvocationSummary`: id, function, trigger (`mode`),
//! environment, build, status, `failed`, the answered HTTP status, duration,
//! the error message and kind, and `has_result`. No request or response body,
//! no held writes (`/held` stays on the console), no call trace.
//!
//! A sandbox agent token is refused: its loop reads invocations through the
//! console route, which already asks it about its own sandbox.
//!
//! Postgres only — FleetOk.

use axum::Json;
use axum::extract::rejection::QueryRejection;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use oxy::database::client::establish_connection;

use oxy_custom_apps::authenticate::authenticate_and_authorize;

use oxy_custom_apps::authenticate::AuthOutcome;
use oxy_custom_apps::env_resolve::may_open_non_production;

use crate::admin::apps::invocations::{
    InvocationQuery, environment_of_build, list_in_environment, rejected,
};

/// The route template an API token's read is counted under.
const INVOCATIONS_ROUTE: &str = "/api/customer-apps/{org_slug}/{app_slug}/invocations";

pub async fn get_invocations(
    Path((org_slug, app_slug)): Path<(String, String)>,
    query: Result<Query<InvocationQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Response {
    let probe = oxy_app_core::usage_probe::UsageProbe::of(&headers);
    let handler = read_invocations(org_slug, app_slug, query, headers);
    probe.counted(INVOCATIONS_ROUTE, handler).await
}

async fn read_invocations(
    org_slug: String,
    app_slug: String,
    query: Result<Query<InvocationQuery>, QueryRejection>,
    headers: HeaderMap,
) -> Response {
    let outcome = match authenticate_and_authorize(
        &headers,
        &org_slug,
        &app_slug,
        oxy_auth::token::SandboxAgent::Refuse,
    )
    .await
    {
        Ok(o) => o,
        Err(status) => return super::error_response(status, "not permitted"),
    };
    // After authentication, so a malformed query says nothing about the app
    // to a caller the resolver would have turned away.
    let q = match query {
        Ok(Query(q)) => q,
        Err(rejection) => return rejected(rejection).into_response(),
    };
    let db = match establish_connection().await {
        Ok(db) => db,
        Err(e) => {
            tracing::error!("db connect failed for the invocation read: {e}");
            return super::error_response(StatusCode::INTERNAL_SERVER_ERROR, "retry shortly");
        }
    };
    let scope = match read_scope(&db, &outcome, &q).await {
        Ok(scope) => scope,
        Err(refused) => return refused,
    };
    let environments = scope.clone().unwrap_or_else(|| ALL.to_string());
    match list_in_environment(&db, outcome.app.id, scope, &q).await {
        Ok(invocations) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "invocations": invocations,
                "environments": environments,
            })),
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

/// What `environments` says when a read covered every environment.
const ALL: &str = "all";

/// Which rows this read returns: one environment's (`Some`, as stored), or
/// every environment's (`None`) — the console's `row_scope`, asked through
/// the `/logs` gate.
///
/// - `?environment=` names one: the logs gate for it, that environment alone.
/// - Otherwise the app-admin gate (production) passes first. A `?build=` that
///   only a non-production environment ran is then asked about as that
///   environment (a `403` without reach) and, admitted, read across
///   environments, as the console does.
/// - Otherwise a caller with non-production reach reads every environment,
///   and one without reads production's.
///
/// Every environment is never more than a named read gives: the logs gate
/// admits a named non-production environment on `may_open_non_production`
/// for the app alone (`environment::log_environment`), whichever environment
/// it is, so a caller who passes it here could read each one by naming it.
async fn read_scope(
    db: &sea_orm::DatabaseConnection,
    outcome: &AuthOutcome,
    q: &InvocationQuery,
) -> Result<Option<String>, Response> {
    if let Some(named) = q.environment.as_deref().filter(|e| !e.trim().is_empty()) {
        let admitted = super::environment::admitted(outcome, Some(named)).await?;
        return Ok(Some(admitted.stored_name()));
    }
    let production = super::environment::admitted(outcome, None).await?;
    if let Some(build) = q.build.as_deref().filter(|b| !b.is_empty()) {
        let serving = environment_of_build(db, &outcome.app, build)
            .await
            .map_err(IntoResponse::into_response)?;
        if let Some(environment) = serving {
            super::environment::admitted(outcome, Some(&environment.name())).await?;
            return Ok(None);
        }
    }
    if may_open_non_production(db, &outcome.caller, &outcome.app).await {
        Ok(None)
    } else {
        Ok(Some(production.stored_name()))
    }
}
