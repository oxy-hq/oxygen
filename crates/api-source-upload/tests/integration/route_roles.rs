//! The upload must stay FleetOk, and its neighbour must not — asserted here
//! rather than in `oxy-app`'s `role_manifest_tests.rs`, where it used to live.
//!
//! It moved because the route did. `oxy-app` cannot depend on this crate, so
//! its harness builds a declaration set without this route, and `classify`
//! would answer FleetOk by default: the assertion would pass for the wrong
//! reason. The guard belongs where both halves are visible, which is here.

use oxy_api_source_upload::route_roles;
use oxy_app::surface::roles::{RouteRole, classify, install_route_declarations_for_tests_with};

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
fn the_upload_is_fleet_ok_and_the_airway_surface_is_not() {
    install();
    assert_eq!(
        classify("POST", &format!("/api/{WORKSPACE}/source-uploads/reports")),
        RouteRole::FleetOk,
        "an S3 write with no working-copy access must not need the ide"
    );

    // The neighbouring surface it deliberately does NOT live under. A start
    // (`/runs`) is a queued task any replica may accept, so the neighbour that
    // proves the point is one still pinned: the chunked backfill drives its
    // chunks in a detached in-process task, which would die with a serve pod.
    assert_eq!(
        classify(
            "POST",
            &format!("/api/{WORKSPACE}/agentic-airway/chunked-backfill")
        ),
        RouteRole::IdeOnly,
        "a chunked backfill is driven in-process and still belongs on the \
         ide — the carve-out is the upload, not the surface"
    );
}
