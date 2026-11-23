//! The upload must stay FleetOk, and its neighbour must not — asserted here
//! rather than in `oxy-app`'s `role_manifest_tests.rs`, where it used to live.
//!
//! It moved because the route did. `oxy-app` cannot depend on this crate, so
//! its harness builds a declaration set without this route, and `classify`
//! would answer FleetOk by default: the assertion would pass for the wrong
//! reason. The guard belongs where both halves are visible: `oxy-api-airway`'s
//! own tests until decompose TR (2026-11-23), `oxy-server`'s since, which
//! depends on both.

use oxy_api_airway::route_roles;
use oxy_app_core::pods::role_manifest::{RouteRole, classify};
use oxy_server::role_manifest::install_route_declarations_for_tests_with;

const WORKSPACE: &str = "22222222-2222-2222-2222-222222222222";

/// Install this crate's declarations the way `oxy-server` does, then ask the
/// same `classify` the request path asks at runtime.
fn install() {
    // Merged INSIDE the `/{workspace_id}` nest, so the declared paths are
    // relative to it — the seam joins the prefix.
    install_route_declarations_for_tests_with(
        route_roles()
            .iter()
            .map(|d| (d.method, format!("/{{workspace_id}}{}", d.path), d.role))
            .collect(),
    );
}

#[test]
fn the_upload_is_fleet_ok() {
    install();
    assert_eq!(
        classify("POST", &format!("/api/{WORKSPACE}/source-uploads/reports")),
        RouteRole::FleetOk,
        "an S3 write with no working-copy access must not need the ide"
    );
}

/// The lease routes sit beside `agentic-http`'s `/agentic-airway` nest and are
/// declared here, by the crate that owns them. Asserted against the declared
/// table itself, so a dropped or flipped declaration fails even though
/// `classify` would default an undeclared route to FleetOk.
#[test]
fn the_lease_routes_are_declared_fleet_ok() {
    for (method, path) in [
        ("GET", "/agentic-airway/leases"),
        ("DELETE", "/agentic-airway/leases/{pipeline_name_b64}"),
    ] {
        let declared = route_roles()
            .iter()
            .find(|d| d.method == method && d.path == path)
            .map(|d| d.role);
        assert_eq!(
            declared,
            Some(RouteRole::FleetOk),
            "{method} {path}: one SELECT or DELETE on airway_pipeline_leases, nothing on disk"
        );
    }
    install();
    assert_eq!(
        classify(
            "DELETE",
            &format!("/api/{WORKSPACE}/agentic-airway/leases/dG9hc3Q")
        ),
        RouteRole::FleetOk
    );
}
