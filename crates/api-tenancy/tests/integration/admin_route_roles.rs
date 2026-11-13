//! The staff console's org and workspace sections are served by the fleet,
//! creating an org included: a new org gets no workspace until its repository
//! is connected, so `POST /admin/orgs` writes only Postgres. Its declarations
//! arrive through `SurfaceSeams::admin`, which `oxy-app`'s harness does not
//! build, so the guard lives here.

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
fn the_whole_org_console_is_served_by_the_fleet() {
    install();
    let id = "d9830be4-c6a4";
    for (method, path) in [
        ("POST", "/api/admin/orgs".to_string()),
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
