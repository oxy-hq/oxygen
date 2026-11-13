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

/// `active-runs`, `tree` and `live` answer from the run rows, and `retry`
/// seeds through the start route's own submit. None reads anything a process
/// holds in memory, so the ops dashboard and its Retry button work with the
/// Factory down.
#[test]
fn the_dashboard_reads_and_retry_need_no_factory() {
    for path in [
        "/coordinator/active-runs",
        "/coordinator/runs/{id}/tree",
        "/coordinator/live",
    ] {
        assert_eq!(
            declared("GET", path),
            Some(RouteRole::FleetOk),
            "GET {path} reads the run rows and must be served by any replica"
        );
    }
    assert_eq!(
        declared("POST", "/coordinator/runs/{id}/retry"),
        Some(RouteRole::FleetOk)
    );
}

/// A chat is started, answered, cancelled and streamed by any pod: a run reads
/// the promoted revision on every role. Phase 3d of factory-retirement.md.
#[test]
fn a_chat_runs_on_any_pod() {
    for (method, path) in [
        ("POST", "/runs"),
        ("GET", "/runs/{id}/events"),
        ("POST", "/runs/{id}/answer"),
        ("POST", "/runs/{id}/cancel"),
    ] {
        assert_eq!(
            declared(method, path),
            Some(RouteRole::FleetOk),
            "{method} {path}"
        );
    }
}

/// A route added without a declaration falls to the one catch-all, the
/// Factory's. The Builder routes are gone: the Builder was removed with the IDE.
#[test]
fn undeclared_routes_stay_on_the_factory_and_the_builder_routes_are_gone() {
    for (method, path) in [
        ("POST", "/builder/runs"),
        ("POST", "/builder/runs/{id}/answer"),
        ("POST", "/runs/{id}/revert-file-changes"),
    ] {
        assert!(
            !router_roles()
                .iter()
                .any(|d| d.method == method && d.path == path),
            "{method} {path} is removed"
        );
    }
    assert_eq!(declared("*", "/{*rest}"), Some(RouteRole::IdeOnly));
    let wildcards = router_roles()
        .iter()
        .filter(|d| d.path.contains("{*"))
        .count();
    assert_eq!(wildcards, 1, "one catch-all, and it is the IdeOnly one");
}

/// The automation routes a customer reaches — starting, cancelling and
/// streaming a run, and reading the automation list and one automation —
/// answer from the compiled revision and the run rows, on any pod.
#[test]
fn an_automation_runs_on_any_pod() {
    use crate::automation_router_roles;
    let declared = |method: &str, path: &str| {
        automation_router_roles()
            .iter()
            .find(|d| d.method == method && d.path == path)
            .map(|d| d.role)
    };
    for (method, path) in [
        ("POST", "/runs"),
        ("POST", "/runs/{id}/cancel"),
        ("GET", "/runs/{id}/events"),
        ("GET", "/files"),
        ("GET", "/files/{path_b64}"),
        ("GET", "/runs"),
        ("GET", "/runs/{id}"),
        ("GET", "/threads/{thread_id}/run"),
    ] {
        assert_eq!(
            declared(method, path),
            Some(RouteRole::FleetOk),
            "{method} {path}"
        );
    }
    assert_eq!(declared("*", "/{*rest}"), Some(RouteRole::IdeOnly));
}

/// A `FleetOk` declaration names its verb. With `*`, carving out a read would
/// also carve out whatever is later mounted at the same path under another
/// method — `GET /coordinator/runs` must not unpin a `POST` there.
#[test]
fn no_fleet_ok_declaration_covers_every_method() {
    for d in router_roles()
        .iter()
        .chain(crate::automation_router_roles())
    {
        if d.role == RouteRole::FleetOk {
            assert_ne!(d.method, "*", "{} is FleetOk for every method", d.path);
        }
    }
}
