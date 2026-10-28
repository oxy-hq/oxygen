//! Which workspace routes are the Factory's, and which are a customer's.
//!
//! Every route under `/api/{workspace_id}/…` is on one of the two lists below, and
//! `oxy-route-catalog`'s `factory_routes` test fails when one is on neither or an
//! entry names no route — so a new route is a decision made here, not a default.
//! At runtime a route that is on neither list is treated as the Factory's
//! ([`surface_of`]): refusing a customer a page is recoverable, handing a customer
//! the IDE is not.
//!
//! **What the line is.** A route is the Factory's when its job is to author or
//! operate the workspace's definitions: the working copy and git, compile, modeling,
//! the semantic layer's editing tools, pipeline and schedule management, tests,
//! databases and integrations setup, secrets, observability, the camera fleet, the
//! world model's exploration tools, onboarding setup, and the working copy's git state
//! and config status. A route is a customer's when a surface a customer reaches calls
//! it: the shell (what the workspace is, its members and API keys), chat and its runs, viewing and running automations and
//! pipelines, the launcher's readiness reads, the world model's
//! graph (the home's preview and the read-only page), and running a query. Reading
//! is not the test — `GET /files` is the Factory's, `POST /sql/query` is a
//! customer's — what the route is *for* is.
//!
//! **Not in the table, so not gated here:**
//!
//! - everything outside `/api/{workspace_id}/…`: the custom-app serving and data plane
//!   (`/customer-apps`, `/projects/{id}/shell-context`), notifications, the user's own
//!   settings, orgs. They never passed through the workspace middleware.
//! - the **external API** (`/external/api/{workspace_id}/…`): a curated surface that
//!   is the customer's programmatic access by design, so [`super::gate`] lets all of it
//!   through whatever it is called.
//! - `/api/{workspace_id}/previews/…`: mounted beside this tree, already staff-only
//!   (`Action::WorkspacePreview`).
//!
//! **Why wildcard entries exist only on the Factory list.** `{*rest}` claims every
//! route under a prefix, so a new route there is the Factory's until someone says
//! otherwise — the safe direction. The customer list names routes exactly, so nothing
//! becomes a customer's by being added next to one.
//!
//! Paths are relative to the workspace root, which is what the workspace middleware
//! sees. A method of `"*"` matches any; an exact method beats it on the same path.

use crate::server::role_manifest::{pattern_matches, specificity};

/// Which side of the line a route is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    /// Authors or operates the workspace: refused to anyone who may not use the
    /// Factory here.
    Factory,
    /// Serves a customer surface: open to everyone with access to the workspace.
    Customer,
}

/// What the lists say about one request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    Listed(Surface),
    /// On neither list.
    Unlisted,
    /// The most specific entry on each list ties — a table bug the catalog test
    /// reports, resolved at runtime as the Factory's.
    Ambiguous,
}

/// Routes a customer's surfaces call. Exact patterns only — see the module doc.
pub const CUSTOMER: &[(&str, &str)] = &[
    // ── The shell, on every page ──────────────────────────────────────────────
    // `/meta` carries `factory_access`, which is how the rail knows. `/git-state` and
    // `/status` are the Factory's: the web app asks for them only when `/meta` says
    // the caller may use the Factory (the shell's git half, the config banner).
    ("GET", "/meta"),
    ("GET", "/logo"),
    ("GET", "/org-subdomain"),
    ("GET", "/builder-availability"),
    // Settings the customer's own officers reach: the activity log, who is in the
    // workspace, API keys and tokens, and the Oxy-access lockdown switch.
    ("GET", "/logs"),
    ("*", "/oxy-access"),
    ("GET", "/members"),
    ("PUT", "/members/{user_id}"),
    ("DELETE", "/members/{user_id}"),
    ("GET", "/api-keys"),
    ("POST", "/api-keys"),
    ("GET", "/api-keys/{id}"),
    ("DELETE", "/api-keys/{id}"),
    ("GET", "/api-keys/{id}/activity"),
    ("POST", "/api-keys/{id}/extend"),
    ("GET", "/api-tokens"),
    // ── Custom apps ───────────────────────────────────────────────────────────
    ("GET", "/custom-apps"),
    ("POST", "/custom-apps/{app_id}/secrets"),
    // ── The launcher's readiness reads ────────────────────────────────────────
    ("GET", "/agents"),
    ("GET", "/databases"),
    ("GET", "/app-integrations"),
    // Connecting the customer's own accounts (a QuickBooks or other OAuth link).
    ("POST", "/integrations/oauth/{provider}/authorize"),
    ("POST", "/integrations/quickbooks/authorize"),
    // ── Chat ──────────────────────────────────────────────────────────────────
    ("*", "/threads"),
    ("*", "/threads/bulk-delete"),
    ("*", "/threads/{id}"),
    ("*", "/threads/{id}/stop"),
    ("POST", "/analytics/runs"),
    ("POST", "/analytics/runs/{id}/answer"),
    ("POST", "/analytics/runs/{id}/cancel"),
    ("GET", "/analytics/runs/{id}/events"),
    // Mobile ask: "tell me when this is answered" on the caller's own run.
    ("POST", "/analytics/runs/{id}/notify"),
    ("PATCH", "/analytics/runs/{id}/thinking_mode"),
    ("GET", "/analytics/threads/{thread_id}/run"),
    ("GET", "/analytics/threads/{thread_id}/runs"),
    // What a thread shows: its artifacts, charts and result files.
    ("GET", "/artifacts/{id}"),
    ("GET", "/charts/{file_path}"),
    ("GET", "/results/files/{file_id}"),
    ("DELETE", "/results/files/{file_id}"),
    // ── Viewing and running automations ───────────────────────────────────────
    // The list and the single definition are served from the compile boundary so
    // the customer nav renders on any replica; running one and reading its runs is
    // what chat's automation picker and the automation page do.
    ("GET", "/automations"),
    ("GET", "/automations/{path_b64}"),
    ("GET", "/procedures"),
    ("GET", "/procedures/{path_b64}"),
    ("GET", "/agentic-automations/files"),
    ("GET", "/agentic-automations/files/{path_b64}"),
    ("GET", "/agentic-automations/runs"),
    ("POST", "/agentic-automations/runs"),
    ("GET", "/agentic-automations/runs/{id}"),
    ("POST", "/agentic-automations/runs/{id}/cancel"),
    ("GET", "/agentic-automations/runs/{id}/events"),
    ("GET", "/agentic-automations/threads/{thread_id}/run"),
    ("GET", "/agentic-workflows/files"),
    ("GET", "/agentic-workflows/files/{path_b64}"),
    ("GET", "/agentic-workflows/runs"),
    ("POST", "/agentic-workflows/runs"),
    ("GET", "/agentic-workflows/runs/{id}"),
    ("POST", "/agentic-workflows/runs/{id}/cancel"),
    ("GET", "/agentic-workflows/runs/{id}/events"),
    ("GET", "/agentic-workflows/threads/{thread_id}/run"),
    // ── Viewing pipelines and their schedules (managing them is the Factory's) ─
    ("GET", "/airway-pipelines"),
    ("GET", "/agentic-airway/runs"),
    ("GET", "/agentic-airway/runs/{id}/events"),
    ("GET", "/agentic-schedules"),
    ("GET", "/agentic-schedules/{id}"),
    // ── Asking the data a question ────────────────────────────────────────────
    ("POST", "/sql/query"),
    ("POST", "/semantic"),
    ("POST", "/semantic/compile"),
    ("GET", "/semantic/preagg-status"),
    ("GET", "/semantic/anomalies"),
    ("POST", "/semantic/anomalies/status"),
    ("POST", "/semantic/anomalies/{anomaly_id}/status"),
    ("POST", "/semantic/anomalies/{anomaly_id}/explain"),
    // ── The world model, read-only ────────────────────────────────────────────
    // The entities, their measures and how they connect: the home's preview card
    // and the read-only world model page. Only the graph itself — instance lookups,
    // filter counts, breakdowns and the live feed stay the Factory's, and so does
    // every write (the model is `.world-model.yml` and the views, written through
    // `/files`).
    ("GET", "/semantic/world-model"),
];

/// Routes that author or operate the workspace. `{*rest}` entries claim a whole
/// prefix; an exact [`CUSTOMER`] entry beneath one still wins for its own path.
pub const FACTORY: &[(&str, &str)] = &[
    // ── The working copy and git ──────────────────────────────────────────────
    ("*", "/abort-rebase"),
    ("*", "/continue-rebase"),
    ("*", "/discard-all"),
    ("*", "/fetch"),
    ("*", "/force-push"),
    ("*", "/pull-changes"),
    ("*", "/push-changes"),
    ("*", "/recent-commits"),
    ("*", "/reset-to-commit"),
    ("*", "/resolve-conflict-file"),
    ("*", "/resolve-conflict-with-content"),
    ("*", "/revision-info"),
    ("*", "/switch-branch"),
    ("*", "/unresolve-conflict-file"),
    ("*", "/worktrees"),
    // The working copy's git state and config validation. The web app's shell asks
    // for them only for a Factory user (`WorkspaceService.getWorkspace`,
    // `useWorkspaceStatus`), so a customer's page sends neither.
    ("GET", "/git-state"),
    ("GET", "/status"),
    ("*", "/branches"),
    ("*", "/branches/{*rest}"),
    ("*", "/files"),
    ("*", "/files/{*rest}"),
    // ── Compile, modeling, tests, simulations ─────────────────────────────────
    ("*", "/compile"),
    ("*", "/compile/{*rest}"),
    ("*", "/modeling"),
    ("*", "/modeling/{*rest}"),
    ("*", "/tests"),
    ("*", "/tests/{*rest}"),
    ("*", "/simulations"),
    ("*", "/simulations/{*rest}"),
    ("*", "/sql/{pathb64}"),
    // ── The semantic layer's editing and operating tools ──────────────────────
    ("*", "/semantic/topic/{file_path_b64}"),
    ("*", "/semantic/view/{file_path_b64}"),
    ("*", "/semantic/preagg-rebuild"),
    ("*", "/semantic/anomalies/scan"),
    ("*", "/semantic/anomalies/preview"),
    ("*", "/semantic/monitors"),
    // The World Model panel's "compared to peers".
    ("*", "/semantic/cohort"),
    ("*", "/semantic/metric-tree"),
    ("*", "/semantic/metric-tree/{*rest}"),
    ("*", "/semantic/world-model"),
    ("*", "/semantic/world-model/{*rest}"),
    // ── Experiments: designed, priced, run and reported in the Factory (#3662) ─
    ("*", "/experiments"),
    ("*", "/experiments/{*rest}"),
    // ── Pipelines, schedules and runs: management and operations ──────────────
    ("*", "/airway-pipelines/{*rest}"),
    ("*", "/agentic-airway/{*rest}"),
    ("*", "/agentic-schedules"),
    ("*", "/agentic-schedules/{*rest}"),
    ("*", "/analytics/coordinator/{*rest}"),
    // The Builder copilot and onboarding: start a Builder run, answer one.
    ("*", "/analytics/builder/{*rest}"),
    ("POST", "/analytics/runs/{id}/revert-file-changes"),
    // ── Databases, integrations, secrets: setup that writes config ────────────
    ("*", "/databases"),
    ("*", "/databases/{*rest}"),
    ("*", "/integrations/{*rest}"),
    ("POST", "/app-integrations"),
    ("DELETE", "/app-integrations/{kind}"),
    ("*", "/secrets"),
    ("*", "/secrets/{*rest}"),
    // ── Onboarding and setup ──────────────────────────────────────────────────
    ("*", "/onboarding/{*rest}"),
    // The setup checks: the Factory's setup wizard and the launcher's "Finish
    // setup" rows, which only staff are shown.
    ("GET", "/onboarding-readiness"),
    ("*", "/setup/{*rest}"),
    ("*", "/source-uploads/{*rest}"),
    // ── Observability ─────────────────────────────────────────────────────────
    ("*", "/traces"),
    ("*", "/traces/{*rest}"),
    ("*", "/metrics/{*rest}"),
    ("*", "/execution-analytics/{*rest}"),
    // ── The camera fleet and the world model ──────────────────────────────────
    ("*", "/cameras"),
    ("*", "/cameras/{*rest}"),
    ("*", "/camera-packs"),
    ("*", "/camera-packs/{*rest}"),
    ("*", "/fleet/{*rest}"),
    ("*", "/world-model/{*rest}"),
];

/// How `method path` is listed. `path` is relative to the workspace root and may
/// carry a trailing slash.
pub fn resolve(method: &str, path: &str) -> Resolution {
    // A HEAD is answered by the GET handler, so it is on the GET route's side.
    let method = if method == "HEAD" { "GET" } else { method };
    let path = match path.trim_end_matches('/') {
        "" => "/",
        trimmed => trimmed,
    };
    let best = |list: &[(&str, &str)]| {
        list.iter()
            .filter(|(m, pattern)| (*m == "*" || *m == method) && pattern_matches(pattern, path))
            .map(|(m, pattern)| specificity(m, pattern))
            .max()
    };
    match (best(CUSTOMER), best(FACTORY)) {
        (None, None) => Resolution::Unlisted,
        (Some(_), None) => Resolution::Listed(Surface::Customer),
        (None, Some(_)) => Resolution::Listed(Surface::Factory),
        (Some(c), Some(f)) if c > f => Resolution::Listed(Surface::Customer),
        (Some(c), Some(f)) if f > c => Resolution::Listed(Surface::Factory),
        (Some(_), Some(_)) => Resolution::Ambiguous,
    }
}

/// The side a request is treated as being on: the table's answer, and the Factory
/// for anything the table does not settle.
pub fn surface_of(method: &str, path: &str) -> Surface {
    match resolve(method, path) {
        Resolution::Listed(surface) => surface,
        Resolution::Unlisted | Resolution::Ambiguous => Surface::Factory,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authoring_routes_are_the_factorys_and_chat_is_a_customers() {
        for (method, path) in [
            ("GET", "/files"),
            ("POST", "/files/abc"),
            ("POST", "/push-changes"),
            ("POST", "/compile"),
            ("GET", "/modeling/proj/lineage"),
            ("PUT", "/secrets/abc"),
            ("POST", "/databases"),
            ("DELETE", "/agentic-schedules/abc"),
            ("GET", "/world-model/events"),
            ("GET", "/cameras/abc/preview/hls/a/b/c.m3u8"),
            // The shell asks for these only for a Factory user.
            ("GET", "/git-state"),
            ("GET", "/status"),
            ("GET", "/onboarding-readiness"),
            ("GET", "/onboarding/github-setup"),
        ] {
            assert_eq!(
                resolve(method, path),
                Resolution::Listed(Surface::Factory),
                "{method} {path}"
            );
        }
        for (method, path) in [
            ("GET", "/meta"),
            ("GET", "/threads"),
            ("POST", "/threads/abc/stop"),
            ("POST", "/analytics/runs"),
            ("GET", "/databases"),
            ("GET", "/agentic-schedules/abc"),
            ("POST", "/sql/query"),
            ("GET", "/custom-apps"),
            ("GET", "/semantic/world-model"),
        ] {
            assert_eq!(
                resolve(method, path),
                Resolution::Listed(Surface::Customer),
                "{method} {path}"
            );
        }
    }

    /// The same path splits by verb: reading the database list is the launcher's,
    /// creating one writes `config.yml`.
    #[test]
    fn an_exact_method_beats_the_wildcard_on_the_same_path() {
        assert_eq!(surface_of("GET", "/databases"), Surface::Customer);
        assert_eq!(surface_of("POST", "/databases"), Surface::Factory);
        assert_eq!(surface_of("GET", "/app-integrations"), Surface::Customer);
        assert_eq!(surface_of("POST", "/app-integrations"), Surface::Factory);
    }

    /// A customer reads the world model's graph; everything else under it, and any
    /// other verb on the graph's own path, stays the Factory's.
    #[test]
    fn a_customer_reads_the_world_model_graph_and_nothing_else_of_it() {
        assert_eq!(
            surface_of("GET", "/semantic/world-model"),
            Surface::Customer
        );
        assert_eq!(
            surface_of("HEAD", "/semantic/world-model"),
            Surface::Customer
        );
        for (method, path) in [
            ("POST", "/semantic/world-model"),
            ("GET", "/semantic/world-model/instances"),
            ("GET", "/semantic/world-model/filter-instances"),
            ("POST", "/semantic/world-model/filter-counts"),
            ("GET", "/semantic/world-model/instance-detail"),
            ("GET", "/semantic/world-model/measure-breakdown"),
            ("GET", "/world-model/events"),
        ] {
            assert_eq!(
                surface_of(method, path),
                Surface::Factory,
                "{method} {path}"
            );
        }
    }

    /// `sql/query` is a literal and `sql/{pathb64}` a parameter: the literal wins, so
    /// running a query is the customer's and running a `.sql` file is not.
    #[test]
    fn a_literal_segment_beats_a_parameter_on_the_same_path() {
        assert_eq!(surface_of("POST", "/sql/query"), Surface::Customer);
        assert_eq!(surface_of("POST", "/sql/queries/abc"), Surface::Factory);
        assert_eq!(surface_of("POST", "/sql/abc"), Surface::Factory);
    }

    #[test]
    fn what_is_on_neither_list_is_treated_as_the_factorys() {
        assert_eq!(
            resolve("GET", "/a-route-nobody-classified"),
            Resolution::Unlisted
        );
        assert_eq!(
            surface_of("GET", "/a-route-nobody-classified"),
            Surface::Factory
        );
    }

    #[test]
    fn a_head_request_is_on_the_side_of_its_get_route() {
        assert_eq!(surface_of("HEAD", "/threads"), Surface::Customer);
        assert_eq!(surface_of("HEAD", "/files/abc"), Surface::Factory);
        assert_eq!(surface_of("HEAD", "/databases"), Surface::Customer);
    }

    #[test]
    fn a_trailing_slash_does_not_change_the_answer() {
        assert_eq!(surface_of("GET", "/threads/"), Surface::Customer);
        assert_eq!(surface_of("GET", "/files/"), Surface::Factory);
    }

    #[test]
    fn no_customer_entry_is_a_wildcard() {
        for (method, pattern) in CUSTOMER {
            assert!(
                !pattern.contains("{*"),
                "{method} {pattern}: a customer route is named exactly, never by prefix"
            );
        }
    }

    #[test]
    fn no_entry_is_listed_twice() {
        let mut seen = std::collections::HashSet::new();
        for entry in CUSTOMER.iter().chain(FACTORY) {
            assert!(seen.insert(*entry), "{entry:?} is listed twice");
        }
    }
}
