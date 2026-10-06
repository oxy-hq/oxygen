//! `GET /api/customer-apps/{org}/{app}/logs` and `/errors` — the read side of
//! per-app debuggability.
//!
//! Two questions an operator asks when an app misbehaves, and until now neither
//! was answerable without SSH:
//!
//! - **What did the server-side code print?** `/logs` — persisted `ctx.log()` /
//!   `console.*` output from Oxy Functions, both modes.
//! - **What did the browser throw?** `/errors` — uncaught client errors grouped
//!   by stack, with **source maps applied server-side** so the frames name real
//!   files instead of `index-a3f2.js:1:48213`.
//!
//! ## Gated on operating the app, not on opening it
//!
//! Authentication is the same inline flow as `/health`, `/debug` and
//! `/availability` — session cookie or bearer token, resolved *before* the app
//! lookup — but authorization is **narrower**. `authenticate_and_authorize`
//! ends at `user_can_access_app`, which for a default-visibility published app
//! is true for every member of the owning org, i.e. the app's ordinary viewers.
//! That is right for the bundle's bytes and wrong for what these two return:
//! `/logs` is the author's server-side `ctx.log()` output, which routinely
//! carries query results and upstream API responses printed while debugging,
//! and `/errors` resolves stacks against source maps, so original file paths
//! and function names. An app's data is what it chose to show a viewer; its log
//! output is not. Both therefore call `require_app_admin`, which goes through
//! `oxy-authz`'s `Ring::AppAdmin`.
//!
//! Both read ClickHouse and nothing else, so **FleetOk** — any replica answers.
//!
//! ## One environment per read
//!
//! `/logs` returns production's lines unless `?environment=<name>` asks for
//! another app environment's, and then it returns that environment's alone.
//! A non-production environment is a staff tool, so naming one needs the
//! app-admin gate above **and** `may_open_non_production` (oxy-authz
//! `Action::AppNonProduction`) — an app admin who is the tenant's own does not
//! read what Oxy staff printed while testing in staging or a sandbox.
//!
//! ## Why resolution happens here rather than in the browser
//!
//! Because the maps are not shipped. `custom_apps_serve::sources::is_source_map`
//! now 404s every `.map`, so the only place a map and a stack meet is on the
//! server, for a caller who has already passed the app-admin gate. See
//! `custom_apps_sourcemap`.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use super::custom_apps_auth::{authenticate_and_authorize, require_app_admin};
use super::custom_apps_sourcemap;

/// Widest window a single read may span, and the most rows it may return.
/// Unbounded observability queries have taken the backend offline before (see
/// product-context's "Observability is ClickHouse-only" gotcha), so both are
/// clamped rather than trusted from the query string.
const MAX_HOURS: u32 = 24 * 7;
const MAX_LIMIT: u32 = 500;

#[derive(Deserialize)]
pub struct LogQuery {
    hours: Option<u32>,
    limit: Option<u32>,
    /// Narrow to one invocation. Empty means every invocation in the window.
    invocation_id: Option<String>,
    /// Narrow to one request (`x-oxy-request-id`), the id a support ticket
    /// quotes. Combines with `invocation_id`.
    request_id: Option<String>,
    /// The app environment whose lines to read: `production` (the default),
    /// `staging` or `dev-<handle>`. One environment per read, never a mix.
    environment: Option<String>,
}

#[derive(Deserialize)]
pub struct ErrorQuery {
    hours: Option<u32>,
    limit: Option<u32>,
    /// Narrow to one build — the usual next question after "when did this
    /// start". Empty means every build in the window.
    build_id: Option<String>,
}

#[derive(Serialize)]
pub struct LogLineResponse {
    pub timestamp: String,
    pub build_id: String,
    pub invocation_id: String,
    pub request_id: String,
    pub function_name: String,
    pub mode: String,
    pub level: String,
    pub seq: u32,
    pub message: String,
    /// The platform-trace id the line was written under, when the process
    /// had one — paste into HyperDX to see the invocation's spans.
    pub trace_id: String,
    /// The app environment the line was written in.
    pub environment: String,
}

#[derive(Serialize)]
pub struct ClientErrorResponse {
    pub stack_hash: String,
    pub error_name: String,
    pub message: String,
    /// Source-mapped where the build's maps allow it, raw where they do not.
    /// Resolution is best-effort per frame, so a stack can be partly resolved.
    pub stack: String,
    /// Whether anything in `stack` actually changed. Without it a reader cannot
    /// tell "these are the real file names" from "the map was missing and this
    /// is still minified", which is exactly the moment they would be misled.
    pub stack_resolved: bool,
    pub build_id: String,
    pub path: String,
    pub kind: String,
    pub occurrences: u64,
    pub sessions: u64,
    pub first_seen: String,
    pub last_seen: String,
}

mod environment;
mod filters;

fn clamp(hours: Option<u32>, limit: Option<u32>) -> (u32, u32) {
    (
        hours.unwrap_or(24).clamp(1, MAX_HOURS),
        limit.unwrap_or(100).clamp(1, MAX_LIMIT),
    )
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}

/// Observability off is not an error — it is the default on a dev box. 501 says
/// the capability is absent, distinct from a 500 that reads as "the query broke".
fn store_or_501()
-> Result<&'static std::sync::Arc<dyn oxy_observability::ObservabilityStore>, Response> {
    oxy_observability::global::get_global().ok_or_else(|| {
        error_response(
            StatusCode::NOT_IMPLEMENTED,
            "observability capture is not configured (OXY_OBSERVABILITY_BACKEND)",
        )
    })
}

pub async fn get_logs(
    Path((org_slug, app_slug)): Path<(String, String)>,
    Query(q): Query<LogQuery>,
    headers: HeaderMap,
) -> Response {
    // An API token's read is counted as its `/api` requests are; a session, an
    // anonymous request and a legacy key run the handler untouched.
    let probe = super::custom_apps_agent::UsageProbe::of(&headers);
    let handler = read_logs(org_slug, app_slug, q, headers);
    probe.counted(LOGS_ROUTE, handler).await
}

/// The route template an API token's log read is counted under.
const LOGS_ROUTE: &str = "/api/customer-apps/{org_slug}/{app_slug}/logs";

async fn read_logs(
    org_slug: String,
    app_slug: String,
    q: LogQuery,
    headers: HeaderMap,
) -> Response {
    let outcome = match authenticate_and_authorize(
        &headers,
        &org_slug,
        &app_slug,
        oxy_auth::token::SandboxAgent::Admit,
    )
    .await
    {
        Ok(o) => o,
        Err(status) => return error_response(status, "not permitted"),
    };
    // Syntax only, so it says nothing about the app to a caller the next gate
    // would turn away — and nothing that is not an id reaches the store.
    let q = match q.with_valid_ids() {
        Ok(q) => q,
        Err(invalid) => return invalid.into_response(),
    };
    // The app-admin gate, then the environment's. Before the store: who may
    // read an environment does not depend on whether this deployment captures
    // logs at all.
    let admitted = match environment::admitted(&outcome, q.environment.as_deref()).await {
        Ok(admitted) => admitted,
        Err(refused) => return refused,
    };
    let store = match store_or_501() {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (hours, limit) = clamp(q.hours, q.limit);

    let rows = match store
        .get_function_logs(
            &outcome.app.org_id.to_string(),
            &outcome.app.id.to_string(),
            hours,
            limit,
            q.invocation_id.as_deref().unwrap_or_default(),
            q.request_id.as_deref().unwrap_or_default(),
            &admitted.environment,
        )
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("function-log query failed for app {}: {e}", outcome.app.id);
            return error_response(StatusCode::BAD_GATEWAY, "log query failed");
        }
    };

    // The store reads by the environment's name. A sandbox agent token is
    // shown its own sandbox's lines, not an earlier one's under that name.
    let logs: Vec<LogLineResponse> = rows
        .into_iter()
        .filter(|r| admitted.shows(&r.timestamp))
        .map(|r| LogLineResponse {
            timestamp: r.timestamp,
            build_id: r.build_id,
            invocation_id: r.invocation_id,
            request_id: r.request_id,
            function_name: r.function_name,
            mode: r.mode,
            level: r.log_level,
            seq: r.seq,
            message: r.message,
            trace_id: r.trace_id,
            environment: r.environment,
        })
        .collect();
    (StatusCode::OK, Json(serde_json::json!({ "logs": logs }))).into_response()
}

pub async fn get_errors(
    Path((org_slug, app_slug)): Path<(String, String)>,
    Query(q): Query<ErrorQuery>,
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
        Err(status) => return error_response(status, "not permitted"),
    };
    // Syntax only, so it says nothing about the app to a caller the next gate
    // would turn away — and nothing that is not an id reaches the store.
    let q = match q.with_valid_ids() {
        Ok(q) => q,
        Err(invalid) => return invalid.into_response(),
    };
    if let Err(status) = require_app_admin(&outcome).await {
        return error_response(status, "app-admin required");
    }
    let store = match store_or_501() {
        Ok(s) => s,
        Err(r) => return r,
    };
    let (hours, limit) = clamp(q.hours, q.limit);

    let groups = match store
        .get_client_errors(
            &outcome.app.org_id.to_string(),
            &outcome.app.id.to_string(),
            hours,
            limit,
            q.build_id.as_deref().unwrap_or_default(),
        )
        .await
    {
        Ok(g) => g,
        Err(e) => {
            tracing::warn!("client-error query failed for app {}: {e}", outcome.app.id);
            return error_response(StatusCode::BAD_GATEWAY, "error query failed");
        }
    };

    // ONE cache across every group in the response. Per-group it was ~3 S3
    // fetches and multi-MB source-map parses per error, up to `MAX_LIMIT`
    // groups — with the frame URLs client-supplied, so an authenticated viewer
    // controlled the multiplier.
    let base_path = format!("/customer-apps/{org_slug}/{app_slug}/");
    let mut maps = custom_apps_sourcemap::MapCache::new();
    let mut errors = Vec::with_capacity(groups.len());
    for g in groups {
        let resolved = custom_apps_sourcemap::resolve_stack(
            outcome.app.id,
            &g.build_id,
            &base_path,
            &g.stack,
            &mut maps,
        )
        .await;
        let stack_resolved = resolved != g.stack;
        errors.push(ClientErrorResponse {
            stack_hash: g.stack_hash,
            error_name: g.error_name,
            message: g.message,
            stack: resolved,
            stack_resolved,
            build_id: g.build_id,
            path: g.path,
            kind: g.kind,
            occurrences: g.occurrences,
            sessions: g.sessions,
            first_seen: g.first_seen,
            last_seen: g.last_seen,
        });
    }
    (
        StatusCode::OK,
        Json(serde_json::json!({ "errors": errors })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unbounded observability scan has taken the backend offline before, so
    /// neither bound may be trusted from the query string.
    #[test]
    fn window_and_limit_are_clamped_not_trusted() {
        assert_eq!(clamp(Some(9_999), Some(9_999)), (MAX_HOURS, MAX_LIMIT));
        assert_eq!(clamp(Some(0), Some(0)), (1, 1));
        assert_eq!(clamp(None, None), (24, 100));
        assert_eq!(clamp(Some(6), Some(50)), (6, 50));
    }
}
