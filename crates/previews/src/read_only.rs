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
//! `oxy_app_core::write_hold::scope`, every connector it gets refuses a statement
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
//!
//! **Where it lives.** The table and its verdicts are here since 2026-11-22
//! (the decompose's P2, out of `oxy-app`'s `server::previews::read_only`,
//! which re-exports them). The axum adapter the workspace middleware calls,
//! `check` (buffering a run's body) and `refusal` (the `409`), stays there
//! beside the middleware: this crate takes the bare `http` crate, not axum.

use http::Method;

use oxy_shared::fleet_role::pattern_matches;

/// Mutating routes a preview may still call. Not because they only read — a
/// route cannot promise that — but because what they execute runs with its
/// writes held (`oxy_app_core::write_hold`), so serving them shows the branch
/// without changing anything.
pub const ALLOWED: &[(&str, &str)] = &[
    // Chat. Starting a run is inspected: a Builder Agent run edits files and
    // onboarding writes the workspace (see `analytics_run_refusal`). An
    // analytics run is served on a held platform: its connectors refuse writes
    // and it gets no automation runner, so it cannot delegate to a branch
    // automation.
    ("POST", "/analytics/runs"),
    ("POST", "/analytics/runs/{id}/answer"),
    ("POST", "/analytics/runs/{id}/cancel"),
    ("PATCH", "/analytics/runs/{id}/thinking_mode"),
    // Asking to hear when a run ends: a flag on the run row. A preview's run
    // is never noticed (`notifications::platform::ask_finished`).
    ("POST", "/analytics/runs/{id}/notify"),
    ("POST", "/threads"),
    ("POST", "/threads/{id}/stop"),
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
    // Peer cohort: two entity-grain reads through the same query executor as
    // the metric-tree analyses, which carries the preview hold.
    ("POST", "/semantic/cohort"),
    // The dry run of one monitor: a time-series read through the same runner
    // as the metric-tree analyses above, with nothing persisted by design. A
    // branch that edits `.monitor.yml` is exactly where someone wants to see
    // what an entry would flag before it is merged. (`/semantic/anomalies/scan`
    // stays refused: it files what it finds.)
    ("POST", "/semantic/anomalies/preview"),
    ("POST", "/semantic/world-model/filter-counts"),
    ("POST", "/simulations/validate"),
    ("POST", "/world-model/competitors"),
    ("POST", "/world-model/foot-traffic/current"),
    ("POST", "/world-model/foot-traffic/radar"),
    ("POST", "/world-model/weather/current"),
    ("POST", "/world-model/llm/messages"),
    ("POST", "/integrations/unifi/preview"),
    // Opening a live camera view.
    ("POST", "/cameras/{cam_id}/preview/webrtc/session"),
    ("POST", "/world-model/camera-stream/{cam_id}/webrtc-session"),
    // Schema browsing and a connectivity probe: nothing is written.
    ("POST", "/databases/inspect"),
    ("POST", "/databases/inspect-schemas"),
    ("POST", "/databases/inspect-schema-tables"),
    // Rendering `.databases/` files for `oxyc sync` (2026-11-21): schema reads
    // returned in the response, nothing written.
    ("POST", "/databases/sync"),
    // A Looker integration's credentials check (`oxyc looker test`): signs in
    // and lists the models, nothing is written.
    ("POST", "/integrations/looker/{name}/test"),
];

const SECRETS: &str = "Changing secrets";

/// Mutating routes a preview refuses, with what the refusal says. `"*"` means
/// any mutating method; reads never reach this table.
pub const REFUSED: &[(&str, &str, &str)] = &[
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
    // Queued for the worker fleet, which runs the promoted revision with no
    // write hold: a preview's run would neither read the preview nor hold
    // its writes.
    ("POST", "/evals/runs", "Running an eval"),
    ("POST", "/experiments", "Creating an experiment"),
    (
        "PUT",
        "/experiments/{name}/design",
        "Editing an experiment design",
    ),
    (
        "POST",
        "/experiments/{name}/duplicate",
        "Duplicating an experiment",
    ),
    (
        "POST",
        "/experiments/{name}/proposals/{proposal_id}/use",
        "Adopting a priced experiment design",
    ),
    (
        "POST",
        "/experiments/{name}/register",
        "Registering an experiment",
    ),
    (
        "POST",
        "/experiments/{name}/retrospective",
        "Analysing an experiment retrospectively",
    ),
    ("POST", "/experiments/{name}/price", "Pricing an experiment"),
    (
        "POST",
        "/experiments/{name}/interim",
        "Reading an experiment's interim estimate",
    ),
    (
        "POST",
        "/experiments/{name}/abort",
        "Aborting an experiment",
    ),
    (
        "*",
        "/semantic/preagg-rebuild",
        "Rebuilding pre-aggregations",
    ),
    (
        "*",
        "/analytics/coordinator/runs/{id}/retry",
        "Retrying a run",
    ),
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
    // Configuration and access.
    ("*", "/secrets", SECRETS),
    ("*", "/secrets/{*rest}", SECRETS),
    ("*", "/custom-apps/{app_id}/secrets", SECRETS),
    ("*", "/shared-secrets/{secret_id}", SECRETS),
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
pub const UNLISTED: &str = "This action";

/// What the table says about one request.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    Allowed,
    Refused(&'static str),
    /// `POST /analytics/runs`: allowed or refused by what the body asks for.
    InspectRunBody,
}

pub fn verdict(method: &Method, path: &str) -> Verdict {
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
pub fn analytics_run_refusal(body: &[u8]) -> Option<&'static str> {
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
pub const RUN_BODY_LIMIT: usize = 1024 * 1024;

#[cfg(test)]
#[path = "read_only_tests.rs"]
mod tests;
