//! `router_roles`, asserted where it is declared.
//!
//! `oxy-app`'s `role_manifest_tests` checks how these *classify* once mounted
//! (prefix, specificity, segment counts). This checks the table itself, so a
//! declaration cannot be dropped or flipped in this crate and only show up in
//! another crate's suite.

use oxy_shared::fleet_role::RouteRole;

use crate::router_roles;

/// The role declared for exactly `method path` — not what the wildcard would
/// give it. `None` means the route has no declaration of its own.
fn declared(method: &str, path: &str) -> Option<RouteRole> {
    router_roles()
        .iter()
        .find(|d| d.method == method && d.path == path)
        .map(|d| d.role)
}

/// What must load with the Factory down: a past conversation, and the three
/// dashboard reads that are SELECTs and nothing else.
#[test]
fn the_postgres_only_reads_need_no_factory() {
    for path in [
        "/threads/{thread_id}/run",
        "/threads/{thread_id}/runs",
        "/coordinator/runs",
        "/coordinator/recovery",
        "/coordinator/queue",
    ] {
        assert_eq!(
            declared("GET", path),
            Some(RouteRole::FleetOk),
            "GET {path} reads Postgres only and must be served by any replica"
        );
    }
}

/// One UPDATE of a row the thread reads load back; no in-process state.
#[test]
fn saving_the_thinking_mode_needs_no_factory() {
    assert_eq!(
        declared("PATCH", "/runs/{id}/thinking_mode"),
        Some(RouteRole::FleetOk)
    );
}

/// `active-runs` and `tree` prefer `RuntimeState::statuses` to the row, and
/// `live` streams nothing else. That map is per process, so these are declared
/// — not left to the wildcard — and a reader of this table for "what is safe to
/// move" finds the reason beside each.
#[test]
fn what_reads_this_processes_run_statuses_is_pinned_by_name() {
    for path in [
        "/coordinator/active-runs",
        "/coordinator/runs/{id}/tree",
        "/coordinator/live",
    ] {
        assert_eq!(
            declared("GET", path),
            Some(RouteRole::IdeOnly),
            "GET {path} reads in-process run statuses and must stay on the ide"
        );
    }
}

/// A write whose airway fallback answers a 500 where the start route answers a
/// retryable 503; pinned until it shares that contract.
#[test]
fn retry_is_pinned_by_name() {
    assert_eq!(
        declared("POST", "/coordinator/runs/{id}/retry"),
        Some(RouteRole::IdeOnly)
    );
}

/// Starting, answering, cancelling, streaming and reverting stay under the
/// catch-all: none may acquire a declaration of its own by accident.
#[test]
fn the_execution_routes_have_no_declaration_but_the_wildcard() {
    for (method, path) in [
        ("POST", "/runs"),
        ("GET", "/runs/{id}/events"),
        ("POST", "/runs/{id}/answer"),
        ("POST", "/runs/{id}/cancel"),
        ("POST", "/runs/{id}/revert-file-changes"),
    ] {
        assert_eq!(declared(method, path), None, "{method} {path}");
    }
    assert_eq!(declared("*", "/{*rest}"), Some(RouteRole::IdeOnly));
    let wildcards = router_roles()
        .iter()
        .filter(|d| d.path.contains("{*"))
        .count();
    assert_eq!(wildcards, 1, "one catch-all, and it is the IdeOnly one");
}

/// A `FleetOk` declaration names its verb. With `*`, carving out a read would
/// also carve out whatever is later mounted at the same path under another
/// method — `GET /coordinator/runs` must not unpin a `POST` there.
#[test]
fn no_fleet_ok_declaration_covers_every_method() {
    for d in router_roles() {
        if d.role == RouteRole::FleetOk {
            assert_ne!(d.method, "*", "{} is FleetOk for every method", d.path);
        }
    }
}
