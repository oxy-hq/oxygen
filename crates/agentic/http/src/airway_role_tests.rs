//! `airway_router_roles`, asserted where it is declared.
//!
//! `oxy-app`'s `role_manifest_tests` checks how these *classify* once mounted
//! (prefix, specificity, the `?branch=` escalation). This checks the table
//! itself, so a declaration cannot be dropped or flipped in this crate and only
//! show up in another crate's suite.

use oxy_shared::fleet_role::RouteRole;

use crate::airway_router_roles;

/// The role declared for exactly `method path` — not what a wildcard would
/// give it. `None` means the route has no declaration of its own.
fn declared(method: &str, path: &str) -> Option<RouteRole> {
    airway_router_roles()
        .iter()
        .find(|d| d.method == method && d.path == path)
        .map(|d| d.role)
}

/// What must work with the Factory down: start, single-window backfill,
/// cancel, the live stream, both resets and the reset's picker — plus the
/// history reads that already did.
#[test]
fn starting_stopping_and_resetting_a_pipeline_need_no_factory() {
    for (method, path) in [
        ("POST", "/runs"),
        ("POST", "/backfill"),
        ("POST", "/runs/{id}/cancel"),
        ("GET", "/runs/{id}/events"),
        ("POST", "/reset-schema"),
        ("POST", "/reset-cursors"),
        ("GET", "/resource-cursors"),
        ("GET", "/runs"),
        ("GET", "/coverage"),
        ("GET", "/backfill-ranges"),
    ] {
        assert_eq!(
            declared(method, path),
            Some(RouteRole::FleetOk),
            "{method} {path} reads and writes Postgres only and must be served by any replica"
        );
    }
}

/// The chunked backfill pair drives its chunks in a detached, non-durable task
/// in the accepting process. Declared — not left to the wildcard — so that
/// widening the wildcard, or reading this table for what is safe to move,
/// cannot put that spawn on a replica that runs no workers.
#[test]
fn the_in_process_backfill_drive_is_pinned_by_name() {
    for path in ["/chunked-backfill", "/resume-backfill"] {
        assert_eq!(
            declared("POST", path),
            Some(RouteRole::IdeOnly),
            "POST {path} spawns a non-durable drive and must stay on the ide"
        );
    }
}

#[test]
fn the_authoring_only_routes_stay_on_the_factory() {
    assert_eq!(declared("GET", "/files"), Some(RouteRole::IdeOnly));
    assert_eq!(
        declared("POST", "/sources/discover"),
        Some(RouteRole::IdeOnly)
    );
}

/// A route added to `airway_router` without a declaration must land on the
/// ide, not default to the fleet.
#[test]
fn an_undeclared_airway_route_falls_to_the_ide() {
    assert_eq!(declared("*", "/{*rest}"), Some(RouteRole::IdeOnly));
    let wildcards = airway_router_roles()
        .iter()
        .filter(|d| d.path.contains("{*"))
        .count();
    assert_eq!(wildcards, 1, "one catch-all, and it is the IdeOnly one");
}
