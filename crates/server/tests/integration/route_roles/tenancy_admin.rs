//! The staff console's org and workspace sections are served by the fleet,
//! creating an org included: a new org gets no workspace until its repository
//! is connected, so `POST /admin/orgs` writes only Postgres. Its declarations
//! arrive through `SurfaceSeams::admin`, which `oxy-app`'s harness does not
//! build, so the guard lived in `oxy-api-tenancy`'s tests, and since decompose
//! TR (2026-11-23) lives here, in `oxy-server`, which depends on both.

use oxy_api_tenancy::admin_sections;
use oxy_app_core::pods::role_manifest::{RouteRole, classify};
use oxy_server::role_manifest::install_route_declarations_for_tests_with;

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
        // An org's subdomain (admin split, PR 1): a Postgres upsert of the
        // `org_subdomains` row, as `oxy-app`'s `org_subdomain_routes_are_fleet_ok`
        // pinned it while the handler was there.
        ("GET", format!("/api/admin/orgs/{id}/subdomain")),
        ("PUT", format!("/api/admin/orgs/{id}/subdomain")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} is Postgres-only and must stay FleetOk"
        );
    }
}
