//! The staff console's org section declares one route IdeOnly — creating an
//! org scaffolds its Default workspace onto node-local disk — and the rest
//! FleetOk. Moved from `oxy-app`'s `role_manifest_tests` with the section: its
//! declaration now arrives through `SurfaceSeams::admin`, which `oxy-app`'s
//! harness does not build, so there `classify` would answer the FleetOk default
//! and the assertion would fail for the wrong reason.

use oxy_api_tenancy::admin_sections;
use oxy_app::surface::roles::{RouteRole, classify, install_route_declarations_for_tests_with};

/// Install the sections' declarations the way the admin seam does — under
/// `/admin` — then ask the same `classify` the request path asks.
fn install() {
    let extra = admin_sections()
        .iter()
        .flat_map(|section| section.decls.clone())
        .map(|d| (d.method, format!("/admin{}", d.path), d.role))
        .collect();
    install_route_declarations_for_tests_with(extra);
}

#[test]
fn admin_create_org_reaches_the_ide_and_the_rest_of_the_console_does_not() {
    install();
    assert_eq!(
        classify("POST", "/api/admin/orgs"),
        RouteRole::IdeOnly,
        "POST admin create-org writes the new org's Default workspace working copy"
    );
    // The carve-out names its verb and its path: the list on the same path,
    // and the rest of the orgs console, stay on the fleet.
    let id = "d9830be4-c6a4";
    for (method, path) in [
        ("GET", "/api/admin/orgs".to_string()),
        ("GET", "/api/admin/orgs-meta".to_string()),
        ("GET", format!("/api/admin/orgs/{id}/detail")),
        ("PATCH", format!("/api/admin/orgs/{id}")),
        ("DELETE", format!("/api/admin/orgs/{id}")),
        ("GET", "/api/admin/workspaces-meta".to_string()),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} is Postgres-only and must stay FleetOk"
        );
    }
}
