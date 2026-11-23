//! Every operating-graph route answers from any replica.
//!
//! These were `route_fleet` in `oxy-app`'s `build_global_routes`, whose state
//! type stated FleetOk. `oxy-app` cannot depend on `oxy-api-operating-graph`,
//! so its own tests held this until decompose TR (2026-11-23) moved it up
//! here, to `oxy-server`, which depends on both. Here both
//! halves are visible: the declarations are installed the way `oxy-server`
//! installs them, then asked through the same `classify` the request path uses.
//!
//! "Assigned to me" pinned to the singleton would take every store's checklist
//! down with each deploy, and a manager could not roster a store while the ide
//! restarts — for Postgres rows.

use oxy_api_operating_graph::route_roles;
use oxy_app::surface::roles::install_route_declarations_for_tests_with;
use oxy_app_core::pods::role_manifest::{RouteRole, classify};

const ORG: &str = "11111111-1111-1111-1111-111111111111";
const ID: &str = "22222222-2222-2222-2222-222222222222";

/// The seam merges at the `/api` root with no prefix, so the declared paths
/// are absolute and the installer only adds `/api`.
fn install() {
    let extra = route_roles()
        .iter()
        .map(|d| (d.method, d.path.to_string(), d.role))
        .collect();
    install_route_declarations_for_tests_with(extra);
}

#[test]
fn every_operating_graph_route_is_served_by_the_fleet() {
    install();
    for (method, path) in [
        ("GET", "/api/work".to_string()),
        ("POST", "/api/work".to_string()),
        ("PATCH", format!("/api/work/{ID}")),
        ("GET", format!("/api/orgs/{ORG}/locations")),
        ("POST", format!("/api/orgs/{ORG}/locations")),
        ("PATCH", format!("/api/orgs/{ORG}/locations/{ID}")),
        (
            "PUT",
            format!("/api/orgs/{ORG}/locations/{ID}/external-ids/toast"),
        ),
        (
            "DELETE",
            format!("/api/orgs/{ORG}/locations/{ID}/external-ids/toast"),
        ),
        ("GET", format!("/api/orgs/{ORG}/roles")),
        ("POST", format!("/api/orgs/{ORG}/roles")),
        ("PATCH", format!("/api/orgs/{ORG}/roles/{ID}")),
        ("DELETE", format!("/api/orgs/{ORG}/roles/{ID}")),
        ("GET", format!("/api/orgs/{ORG}/assignments")),
        ("POST", format!("/api/orgs/{ORG}/assignments")),
        ("DELETE", format!("/api/orgs/{ORG}/assignments/{ID}")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} reads and writes Postgres only",
        );
    }
}
