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

/// The chunked backfill pair queues its drive for the worker fleet
/// (`agentic_pipeline::backfill_task`), so any replica may accept it.
#[test]
fn the_backfill_drives_are_served_by_any_replica() {
    for path in ["/chunked-backfill", "/resume-backfill"] {
        assert_eq!(
            declared("POST", path),
            Some(RouteRole::FleetOk),
            "POST {path} only records the range and queues its drive"
        );
    }
}

/// No catch-all and no pin: nothing on the airway surface needs the ide.
#[test]
fn nothing_on_the_airway_surface_is_pinned() {
    let pinned: Vec<&str> = airway_router_roles()
        .iter()
        .filter(|d| d.role == RouteRole::IdeOnly)
        .map(|d| d.path)
        .collect();
    assert!(pinned.is_empty(), "pinned to the ide: {pinned:?}");
}
