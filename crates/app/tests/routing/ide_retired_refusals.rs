//! With the `ide_retired` flag on, which routes a deployment refuses.
//!
//! The refusal is keyed on the role the router declares, so this drives the
//! decision (`factory_access::ide_retired::refuses`) over the declarations a
//! real build produces (`route_declarations()`), the way `enforce_role` and
//! `workspace_middleware` resolve them. No database, no flag cache: the flag's
//! on/off read is the callers' first line, and its off arm runs nothing.
//!
//! What is pinned:
//!
//! - **No `FleetOk` route is refused**, so turning the flag on cannot take away
//!   anything a replica serves today.
//! - **Every `IdeOnly` route of the workspace tree is refused unless `KEPT`
//!   names it**, and the named file, git, compile, config-write and Builder
//!   routes are among the refused.
//! - **The kept reads, data flow and chat stay**, each by name.
//! - **Every `KEPT` entry still matches a declaration** (bar the two
//!   onboarding ones, which `oxy-api-tenancy` declares at the composition root),
//!   so the list cannot outlive the routes it excuses.

use oxy_app::server::factory_access::ide_retired::{KEPT, refuses};
use oxy_app::server::role_manifest::{RouteRole, declared_role_in, pattern_matches};

const WORKSPACE: &str = "/api/{workspace_id}";
const A_WORKSPACE: &str = "7a4e3c1e-0b5f-4c36-9d55-3f1c2b8a9e01";

type Decls = Vec<(&'static str, String, RouteRole)>;

fn declarations() -> Decls {
    oxy_app::server::router::route_declarations()
}

/// A request path a declaration pattern matches: the workspace id a real one,
/// every other parameter and the rest wildcard a placeholder segment.
fn concrete(pattern: &str) -> String {
    pattern
        .split('/')
        .map(|segment| match segment {
            "{workspace_id}" => A_WORKSPACE,
            s if s.starts_with('{') => "x",
            s => s,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn method_for(declared: &str) -> &str {
    if declared == "*" { "GET" } else { declared }
}

/// Whether the retired IDE refuses a request at `method pattern`, resolved
/// against every declaration as the router would.
fn refused(decls: &Decls, method: &str, pattern: &str) -> bool {
    let method = method_for(method);
    let path = concrete(pattern);
    let role = declared_role_in(decls, method, &path).unwrap_or(RouteRole::FleetOk);
    refuses(method, &path, role)
}

fn declared(decls: &Decls, method: &str, relative: &str) -> String {
    let pattern = format!("{WORKSPACE}{relative}");
    assert!(
        decls.iter().any(|(m, p, _)| *m == method && *p == pattern),
        "no declaration `{method} {pattern}`: the route moved; update this list"
    );
    pattern
}

#[test]
fn no_fleet_route_is_refused() {
    let decls = declarations();
    for (method, pattern, role) in &decls {
        let path = concrete(pattern);
        let effective = declared_role_in(&decls, method_for(method), &path);
        if *role == RouteRole::FleetOk && effective == Some(RouteRole::FleetOk) {
            assert!(
                !refused(&decls, method, pattern),
                "FleetOk `{method} {pattern}` must not be refused with the IDE retired"
            );
        }
    }
}

#[test]
fn file_git_compile_config_and_builder_routes_are_refused() {
    let decls = declarations();
    for (method, relative) in [
        ("*", "/files"),
        ("*", "/files/{pathb64}"),
        ("*", "/files/{pathb64}/delete-file"),
        ("*", "/files/{pathb64}/rename-file"),
        ("*", "/files/diff-summary"),
        ("*", "/git-state"),
        ("*", "/status"),
        ("*", "/branches"),
        ("*", "/switch-branch"),
        ("*", "/push-changes"),
        ("*", "/pull-changes"),
        ("*", "/revision-info"),
        ("*", "/reset-to-commit"),
        ("*", "/compile"),
        ("POST", "/databases"),
        ("*", "/databases/sync"),
        ("*", "/databases/build"),
        ("*", "/databases/clean"),
        ("POST", "/app-integrations"),
        ("*", "/app-integrations/{kind}"),
        ("POST", "/analytics/builder/runs"),
        ("POST", "/analytics/builder/runs/{id}/answer"),
        ("POST", "/analytics/runs/{id}/revert-file-changes"),
    ] {
        let pattern = declared(&decls, method, relative);
        assert!(
            refused(&decls, method, &pattern),
            "`{method} {pattern}` edits files, runs git or compiles the working copy: refuse it"
        );
    }
}

#[test]
fn kept_reads_data_flow_and_chat_stay() {
    let decls = declarations();
    for (method, relative) in [
        ("*", "/compile/status"),
        ("*", "/semantic/anomalies/scan"),
        ("*", "/databases/test-connection"),
        ("*", "/integrations/looker"),
        ("POST", "/agentic-airway/chunked-backfill"),
        ("POST", "/agentic-airway/resume-backfill"),
        ("POST", "/agentic-airway/sources/discover"),
        ("*", "/agentic-airway/{*rest}"),
        ("*", "/agentic-automations/{*rest}"),
        ("*", "/analytics/{*rest}"),
    ] {
        let pattern = declared(&decls, method, relative);
        assert!(
            !refused(&decls, method, &pattern),
            "`{method} {pattern}` is kept with the IDE retired"
        );
    }
}

#[test]
fn every_kept_entry_still_names_an_ide_only_route() {
    let decls = declarations();
    for (method, relative, why) in KEPT {
        if relative.starts_with("/onboarding") {
            // Declared by `oxy-api-tenancy`, merged at the composition root.
            continue;
        }
        let matched = decls.iter().any(|(m, pattern, role)| {
            *role == RouteRole::IdeOnly
                && (*method == "*" || *m == *method || *m == "*")
                && pattern
                    .strip_prefix(WORKSPACE)
                    .is_some_and(|rel| rel == *relative || pattern_matches(relative, rel))
        });
        assert!(
            matched,
            "KEPT `{method} {relative}` ({why}) names no IdeOnly route any more; delete it"
        );
    }
}
