//! A preview can be read, not run or changed.
//!
//! A request pinned to a preview revision is refused with `409
//! preview_read_only` when it would run something or change something: its
//! definitions are unmerged, so running them against real data would make the
//! change live in effect, and a change made while looking at a preview lands on
//! the real workspace, not the branch.
//!
//! **"Read-only" means the workspace and its data — not Oxy's own
//! control-plane tables.** [`ALLOWED`] includes `POST /threads` and `POST
//! /analytics/runs`, and both write ordinary Oxy rows (a thread, a run). A
//! thread or run row created while pinned to a preview is that rule working,
//! not a leak of the guarantee — the user-facing copy says "can't change the
//! workspace or its data" rather than a blanket "it's read-only" for exactly
//! this reason.
//!
//! **This table is the first gate, not the guarantee.** A route cannot say
//! whether what it runs only reads: `/sql/*` runs the SQL it is sent, a data
//! app runs the branch's task SQL and HTTP calls, and chat can reach for a
//! branch automation. So [`ALLOWED`] is not "these only read"; it is "these
//! are worth serving, and what they execute is held": the request runs inside
//! [`super::request_hold::scope`], every connector it gets refuses a statement
//! that is not a read, an `http_request` step sends only `GET`/`HEAD`, no
//! automation runner or builder bridge is built, and an Airway step is
//! refused. A route whose writes the execution layer cannot hold belongs on
//! [`REFUSED`].
//!
//! **Deny by default.** A mutating request is refused unless it is on
//! [`ALLOWED`] — the query-shaped `POST`s (SQL, semantic queries, metric-tree
//! analyses, rendering a data app, chat), served with their writes held.
//! [`REFUSED`] gives the rest a sentence the frontend can show; a mutating
//! route on neither list is refused as "This action". `GET` / `HEAD` /
//! `OPTIONS` are reads and always pass.
//!
//! Paths are relative to the workspace root (`/api/{workspace_id}`), which is
//! what the workspace middleware sees. `every_mutating_route_is_classified`
//! walks the generated route catalog so a new mutating route has to be put on
//! one list or the other.

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};

use super::pin::PreviewPin;
use crate::server::role_manifest::pattern_matches;

/// Mutating routes a preview may still call. Not because they only read — a
/// route cannot promise that — but because what they execute runs with its
/// writes held (`super::request_hold`), so serving them shows the branch
/// without changing anything.
pub(crate) const ALLOWED: &[(&str, &str)] = &[
    // Chat. Starting a run is inspected: a Builder Agent run edits files and
    // onboarding writes the workspace (see `analytics_run_refusal`). An
    // analytics run is served on a held platform: its connectors refuse writes
    // and it gets no automation runner, so it cannot delegate to a branch
    // automation.
    ("POST", "/analytics/runs"),
    ("POST", "/analytics/runs/{id}/answer"),
    ("POST", "/analytics/runs/{id}/cancel"),
    ("PATCH", "/analytics/runs/{id}/thinking_mode"),
    ("POST", "/threads"),
    ("POST", "/threads/{id}/stop"),
    // Rendering a data app runs the branch's tasks: task SQL that is not a
    // read is refused at the connector, and an `http_request` task sends only
    // GET/HEAD.
    ("POST", "/apps/{pathb64}/run"),
    ("POST", "/apps/{pathb64}/result"),
    // Queries. The SQL is the caller's (or a branch `.sql` file's), so a
    // statement that is not a read is refused at the connector.
    ("POST", "/sql/query"),
    ("POST", "/sql/{pathb64}"),
    ("POST", "/semantic"),
    ("POST", "/semantic/compile"),
    ("POST", "/semantic/metric-tree/baseline"),
    ("POST", "/semantic/metric-tree/distribution"),
    ("POST", "/semantic/metric-tree/drill"),
    ("POST", "/semantic/metric-tree/explain"),
    ("POST", "/semantic/metric-tree/opportunity"),
    ("POST", "/semantic/metric-tree/predict"),
    ("POST", "/semantic/metric-tree/projection"),
    ("POST", "/semantic/world-model/filter-counts"),
    ("POST", "/simulations/validate"),
    ("POST", "/world-model/competitors"),
    ("POST", "/world-model/foot-traffic/current"),
    ("POST", "/world-model/foot-traffic/radar"),
    ("POST", "/world-model/weather/current"),
    ("POST", "/world-model/llm/messages"),
    ("POST", "/integrations/looker/query"),
    ("POST", "/integrations/looker/query/sql"),
    ("POST", "/integrations/unifi/preview"),
    // Opening a live camera view.
    ("POST", "/cameras/{cam_id}/preview/webrtc/session"),
    ("POST", "/world-model/camera-stream/{cam_id}/webrtc-session"),
    // Schema browsing and a connectivity probe: nothing is written.
    ("POST", "/databases/inspect"),
    ("POST", "/databases/inspect-schemas"),
    ("POST", "/databases/inspect-schema-tables"),
    ("POST", "/databases/test-connection"),
];

const GIT: &str = "Changing the workspace's git state";
const SECRETS: &str = "Changing secrets";

/// Mutating routes a preview refuses, with what the refusal says. `"*"` means
/// any mutating method; reads never reach this table.
pub(crate) const REFUSED: &[(&str, &str, &str)] = &[
    // Runs.
    ("*", "/agentic-automations/{*rest}", "Running an automation"),
    ("*", "/agentic-workflows/{*rest}", "Running an automation"),
    ("*", "/agentic-airway/{*rest}", "Running an Airway pipeline"),
    ("*", "/semantic/anomalies/scan", "Running a monitor scan"),
    (
        "*",
        "/semantic/anomalies/{anomaly_id}/explain",
        "Explaining an anomaly",
    ),
    ("*", "/simulations/{name}/runs", "Running a simulation"),
    (
        "*",
        "/semantic/preagg-rebuild",
        "Rebuilding pre-aggregations",
    ),
    ("*", "/modeling/{*rest}", "Running Airform modeling"),
    ("*", "/tests/{*rest}", "Running tests"),
    (
        "*",
        "/analytics/coordinator/runs/{id}/retry",
        "Retrying a run",
    ),
    ("*", "/compile", "Compiling the workspace"),
    ("*", "/compile/staging", "Compiling the workspace"),
    // Schedules.
    ("POST", "/agentic-schedules", "Creating a schedule"),
    ("PATCH", "/agentic-schedules/{id}", "Changing a schedule"),
    ("DELETE", "/agentic-schedules/{id}", "Deleting a schedule"),
    ("*", "/agentic-schedules/{id}/run-now", "Running a schedule"),
    (
        "*",
        "/agentic-schedules/{id}/backfill",
        "Backfilling a schedule",
    ),
    // Files and the Builder Agent's edits.
    ("*", "/files/{*rest}", "Editing files"),
    (
        "*",
        "/analytics/runs/{id}/revert-file-changes",
        "Reverting the Builder Agent's changes",
    ),
    ("*", "/apps/save-from-run/{run_id}", "Saving a data app"),
    ("*", "/apps/{pathb64}/publish", "Publishing a data app"),
    ("*", "/apps/{pathb64}/unpublish", "Unpublishing a data app"),
    ("*", "/app-integrations", "Changing an app integration"),
    (
        "*",
        "/app-integrations/{kind}",
        "Changing an app integration",
    ),
    // Git.
    ("*", "/abort-rebase", GIT),
    ("*", "/continue-rebase", GIT),
    ("*", "/discard-all", GIT),
    ("*", "/fetch", GIT),
    ("*", "/force-push", GIT),
    ("*", "/pull-changes", GIT),
    ("*", "/push-changes", GIT),
    ("*", "/reset-to-commit", GIT),
    ("*", "/resolve-conflict-file", GIT),
    ("*", "/resolve-conflict-with-content", GIT),
    ("*", "/unresolve-conflict-file", GIT),
    ("*", "/switch-branch", GIT),
    ("*", "/branches/{branch_name}", GIT),
    ("*", "/repositories", "Changing data repositories"),
    ("*", "/repositories/{*rest}", "Changing data repositories"),
    // Configuration and access.
    ("*", "/databases", "Adding a database"),
    ("*", "/databases/sync", "Syncing a database"),
    ("*", "/databases/build", "Building embeddings"),
    ("*", "/databases/clean", "Cleaning database data"),
    ("*", "/secrets", SECRETS),
    ("*", "/secrets/{*rest}", SECRETS),
    ("*", "/custom-apps/{app_id}/secrets", SECRETS),
    ("*", "/api-keys", "Managing API keys"),
    ("*", "/api-keys/{id}", "Managing API keys"),
    ("*", "/members/{user_id}", "Changing workspace members"),
    ("*", "/oxy-access", "Changing Oxy access"),
    (
        "*",
        "/integrations/oauth/{provider}/authorize",
        "Connecting an integration",
    ),
    (
        "*",
        "/integrations/quickbooks/authorize",
        "Connecting an integration",
    ),
    ("*", "/integrations/unifi/import", "Importing from UniFi"),
    ("*", "/setup/demo", "Setting up the workspace"),
    ("*", "/setup/empty", "Setting up the workspace"),
    ("*", "/onboarding/{*rest}", "Onboarding"),
    ("*", "/source-uploads/reports", "Uploading a report"),
    // Cameras and the edge fleet.
    ("*", "/cameras", "Changing cameras"),
    ("*", "/cameras/{*rest}", "Changing cameras"),
    ("*", "/camera-packs", "Changing cameras"),
    ("*", "/camera-packs/{*rest}", "Changing cameras"),
    ("*", "/fleet/{*rest}", "Changing the camera fleet"),
    // Triage and history.
    ("*", "/semantic/anomalies/status", "Triaging anomalies"),
    (
        "*",
        "/semantic/anomalies/{anomaly_id}/status",
        "Triaging anomalies",
    ),
    ("*", "/threads", "Deleting threads"),
    ("*", "/threads/bulk-delete", "Deleting threads"),
    ("*", "/threads/{id}", "Deleting threads"),
    ("*", "/results/files/{file_id}", "Deleting a result file"),
];

/// What a refusal says about a mutating route on neither list.
const UNLISTED: &str = "This action";

/// What the table says about one request.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Allowed,
    Refused(&'static str),
    /// `POST /analytics/runs`: allowed or refused by what the body asks for.
    InspectRunBody,
}

pub(crate) fn verdict(method: &Method, path: &str) -> Verdict {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return Verdict::Allowed;
    }
    let path = normalise(path);
    if method == Method::POST && path == "/analytics/runs" {
        return Verdict::InspectRunBody;
    }
    let m = method.as_str();
    if ALLOWED
        .iter()
        .any(|(am, p)| *am == m && pattern_matches(p, path))
    {
        return Verdict::Allowed;
    }
    let what = REFUSED
        .iter()
        .find(|(rm, p, _)| (*rm == "*" || *rm == m) && pattern_matches(p, path))
        .map(|(_, _, what)| *what)
        .unwrap_or(UNLISTED);
    Verdict::Refused(what)
}

/// Trailing slashes don't change the route: `/agentic-schedules/` is the same
/// mount as `/agentic-schedules`.
fn normalise(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() { "/" } else { trimmed }
}

/// A run body that asks for the Builder Agent (it edits files) or the
/// onboarding flow (it writes the workspace) is refused; an analytics run —
/// chat that only queries — is not. A body that isn't a run request is left
/// for the handler to reject.
pub(crate) fn analytics_run_refusal(body: &[u8]) -> Option<&'static str> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    if v.get("domain").and_then(|d| d.as_str()) == Some("builder") {
        return Some("Building with the Builder Agent");
    }
    if v.get("onboarding_context").is_some_and(|c| !c.is_null()) {
        return Some("Onboarding");
    }
    None
}

/// A run request's body is small (a question and a few ids). Anything past this
/// is not one, and is refused rather than buffered.
const RUN_BODY_LIMIT: usize = 1024 * 1024;

/// Check a request pinned to `pin`. `Ok` hands the request back (its body
/// re-assembled if it had to be read); `Err` is the 409 to return instead.
pub async fn check(request: Request<Body>, pin: &PreviewPin) -> Result<Request<Body>, Response> {
    match verdict(request.method(), request.uri().path()) {
        Verdict::Allowed => Ok(request),
        Verdict::Refused(what) => Err(refusal(what, pin)),
        Verdict::InspectRunBody => {
            let (parts, body) = request.into_parts();
            let Ok(bytes) = axum::body::to_bytes(body, RUN_BODY_LIMIT).await else {
                return Err(refusal(UNLISTED, pin));
            };
            match analytics_run_refusal(&bytes) {
                Some(what) => Err(refusal(what, pin)),
                None => Ok(Request::from_parts(parts, Body::from(bytes))),
            }
        }
    }
}

/// `409 {"code":"preview_read_only","message":"<what> isn't available in a
/// preview; merge the branch to run it"}`, stamped like any preview response.
pub fn refusal(what: &str, pin: &PreviewPin) -> Response {
    let mut response = (
        StatusCode::CONFLICT,
        axum::Json(serde_json::json!({
            "code": "preview_read_only",
            "message": format!("{what} isn't available in a preview; merge the branch to run it"),
        })),
    )
        .into_response();
    pin.stamp(&mut response);
    response
}

#[cfg(test)]
#[path = "read_only_tests.rs"]
mod tests;
