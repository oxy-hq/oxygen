//! The `ide_retired` rollout gate on the server: which routes a deployment that
//! has retired the in-product IDE refuses, and what it answers.
//!
//! **Keyed on the declared role.** A route is refused when the role manifest
//! classifies it `IdeOnly`, it sits in the workspace tree (`/api/{workspace_id}/…`
//! or the external mirror), and it is not on [`KEPT`]. So the whole working-copy
//! surface — file editing, git and branches, the Compile button, the Database
//! client's sync/build/clean, config writes, the Builder — goes with one switch,
//! and a new `IdeOnly` route is refused by default.
//!
//! **Where the answer is made.** Two places, one decision ([`refuses`]):
//!
//! - `role_middleware::enforce_role` runs before authentication. With the flag on
//!   it never forwards a refused route to the Factory and never escalates a
//!   `?branch=` request there: it serves the request on this pod, so
//! - `workspace_middleware`, after it has authorized the caller for the workspace
//!   (and after the Factory-access gate), answers `410 Gone` from [`refusal_for`],
//!   naming the workspace's repository. The repository is never told to a caller
//!   the workspace did not authorize, which is why the answer is not made earlier.
//!
//! Routes outside the workspace tree (org onboarding, staff and partner org
//! creation, workspace deletion, `/documents/ask`) are not the IDE and are never
//! refused here. Local mode (`--local`) does not run `workspace_middleware`.
//!
//! With the flag off nothing here runs: [`is_on`] is the first thing both callers
//! ask. `internal-docs/factory-retirement.md`, phase 5.

use axum::body::Body;
use axum::extract::OriginalUri;
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response};
use entity::workspaces;

use crate::server::feature_flags::rollout::IDE_RETIRED_FLAG;
use crate::server::role_manifest::{RouteRole, classify, pattern_matches, specificity};

/// The `error` / `code` of the 410 body, which the web app keys on.
pub const REFUSAL_CODE: &str = "ide_retired";

/// `IdeOnly` routes of the workspace tree that stay with the flag on, each with
/// why: a read the non-IDE surfaces call, data flow, chat and runs, or
/// provisioning. Paths are relative to the workspace root. Everything else that
/// is `IdeOnly` there edits files, runs git or compiles the working copy.
///
/// Onboarding's provisioning routes stay; its agentic setup does not. That step
/// is a run of the Builder (`POST /analytics/builder/runs`), which
/// [`REFUSED_BENEATH_KEPT`] refuses, and the web app hides it with the flag on:
/// a workspace is set up in its repository (phase 6: a new org gets no
/// workspace until one is connected).
pub const KEPT: &[(&str, &str, &str)] = &[
    (
        "*",
        "/compile/status",
        "a Postgres read of the compile queue and the served revision",
    ),
    (
        "*",
        "/semantic/anomalies/scan",
        "data flow: a monitor scan (phase 4 moves it onto the revision)",
    ),
    (
        "*",
        "/databases/test-connection",
        "dials the warehouse the request describes; Settings and onboarding call it",
    ),
    (
        "*",
        "/integrations/looker",
        "lists Looker integrations for the automation form's looker_query step",
    ),
    (
        "*",
        "/agentic-airway/{*rest}",
        "data flow: chunked backfill, resume and source discovery",
    ),
    (
        "*",
        "/agentic-automations/{*rest}",
        "automation runs (phase 3)",
    ),
    (
        "*",
        "/agentic-workflows/{*rest}",
        "agentic-workflow runs (phase 3)",
    ),
    (
        "*",
        "/analytics/{*rest}",
        "chat and its runs (phase 3); the Builder beneath is refused",
    ),
    (
        "*",
        "/onboarding/{*rest}",
        "provisioning, not the IDE (phase 6)",
    ),
    (
        "GET",
        "/onboarding-readiness",
        "provisioning, not the IDE (phase 6)",
    ),
];

/// Beneath a [`KEPT`] prefix, the routes that are the IDE's all the same: the
/// Builder copilot, which edits files (onboarding's agentic setup included), and
/// the revert of its edits.
pub const REFUSED_BENEATH_KEPT: &[(&str, &str)] = &[
    ("POST", "/analytics/builder/{*rest}"),
    ("POST", "/analytics/runs/{id}/revert-file-changes"),
];

/// Whether this deployment has retired the IDE.
pub fn is_on() -> bool {
    crate::server::feature_flags::is_enabled(IDE_RETIRED_FLAG)
}

/// Whether a retired IDE refuses `method path`, a route whose declared role is
/// `role`. `path` is the full request path (`/api/{uuid}/files`), as
/// `role_manifest::classify` takes it. Pure: the caller asks [`is_on`] first.
pub fn refuses(method: &str, path: &str, role: RouteRole) -> bool {
    if role != RouteRole::IdeOnly {
        return false;
    }
    match workspace_relative(path) {
        Some(relative) => !kept(method, relative),
        None => false,
    }
}

/// [`refuses`] for a request, classifying it the way `enforce_role` does.
pub fn refuses_request(method: &str, path: &str) -> bool {
    refuses(method, path, classify(method, path))
}

/// The remainder after `/api/{uuid}` (or `/external/api/{uuid}`), or `None` for
/// a path outside the workspace tree.
fn workspace_relative(path: &str) -> Option<&str> {
    let rest = path
        .strip_prefix("/external/api/")
        .or_else(|| path.strip_prefix("/api/"))?;
    let workspace = rest.split('/').next().unwrap_or(rest);
    uuid::Uuid::parse_str(workspace).ok()?;
    match rest[workspace.len()..].trim_end_matches('/') {
        "" => Some("/"),
        relative => Some(relative),
    }
}

/// Whether `method relative` is on [`KEPT`] and not beneath it on
/// [`REFUSED_BENEATH_KEPT`]; the more specific entry decides.
fn kept(method: &str, relative: &str) -> bool {
    let method = if method == "HEAD" { "GET" } else { method };
    let keep = best_rank(KEPT.iter().map(|(m, p, _)| (*m, *p)), method, relative);
    let refuse = best_rank(REFUSED_BENEATH_KEPT.iter().copied(), method, relative);
    match (keep, refuse) {
        (Some(k), Some(r)) => k > r,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

/// The specificity of the most specific entry matching `method relative`.
fn best_rank<'a>(
    entries: impl Iterator<Item = (&'a str, &'a str)>,
    method: &str,
    relative: &str,
) -> Option<u32> {
    entries
        .filter(|(m, pattern)| (*m == "*" || *m == method) && pattern_matches(pattern, relative))
        .map(|(m, pattern)| specificity(m, pattern))
        .max()
}

/// The 410 for `request` when the flag is on and its route is refused, built
/// from the workspace `workspace_middleware` attached. `None` lets it through.
pub fn refusal_for(request: &Request<Body>) -> Option<Response> {
    if !is_on() {
        return None;
    }
    let path = request
        .extensions()
        .get::<OriginalUri>()
        .map(|original| original.0.path().to_string())
        .unwrap_or_else(|| request.uri().path().to_string());
    if !refuses_request(request.method().as_str(), &path) {
        return None;
    }
    tracing::info!(
        method = %request.method(),
        path = %path,
        "ide_retired: refused an IDE route"
    );
    Some(gone(request.extensions().get::<workspaces::Model>()))
}

/// Where to work on a workspace instead of the IDE.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Repository {
    pub repo_url: Option<String>,
    pub default_branch: Option<String>,
    pub repo_subdir: Option<String>,
}

impl Repository {
    /// The facts `/meta` serves, through the same helper, so the remote is
    /// stripped of credentials in one place.
    pub fn of(workspace: &workspaces::Model) -> Self {
        let (repo_url, default_branch, repo_subdir) =
            crate::server::api::workspaces::workspace_repository_facts(workspace);
        Self {
            repo_url,
            default_branch,
            repo_subdir,
        }
    }

    /// `git clone --branch '<default>' -- '<url>'`, or `None` with no remote.
    /// Both values come from the workspace row and people paste the line into a
    /// shell, so each is single-quoted.
    pub fn clone_command(&self) -> Option<String> {
        let url = shell_quote(self.repo_url.as_deref()?);
        Some(match self.default_branch.as_deref() {
            Some(branch) => format!("git clone --branch {} -- {url}", shell_quote(branch)),
            None => format!("git clone -- {url}"),
        })
    }
}

/// `value` as one POSIX shell word: single-quoted, an embedded `'` as `'\''`.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// The `410 Gone` body: what was refused and where the repository is.
pub fn gone(workspace: Option<&workspaces::Model>) -> Response {
    let repository = workspace.map(Repository::of).unwrap_or_default();
    let (message, hint) = match repository.clone_command() {
        Some(clone) => (
            "The in-product IDE is retired on this deployment. Work on this workspace in its \
             repository with your own editor or an agent: push to the default branch and the \
             change compiles and deploys.",
            clone,
        ),
        None => (
            "The in-product IDE is retired on this deployment, and this workspace has no \
             repository. Connect one in Settings, then work on it with your own editor.",
            "Settings -> connect a repository".to_string(),
        ),
    };
    (
        StatusCode::GONE,
        axum::Json(serde_json::json!({
            "error": REFUSAL_CODE,
            "code": REFUSAL_CODE,
            "message": message,
            "repo_url": repository.repo_url,
            "default_branch": repository.default_branch,
            "repo_subdir": repository.repo_subdir,
            "hint": hint,
        })),
    )
        .into_response()
}

#[cfg(test)]
#[path = "ide_retired_tests.rs"]
mod tests;
