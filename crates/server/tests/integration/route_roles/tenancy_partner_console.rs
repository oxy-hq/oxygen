//! The partner console's routes are served by the fleet.
//!
//! Moved here from the `#[cfg(test)]` module of `oxy-api-tenancy`'s
//! `src/partner_console/route_roles.rs` (decompose TR, 2026-11-23), so that
//! crate no longer needs `oxy-app` to test it.

use oxy_api_tenancy::partner_console::route_roles;
use oxy_app::surface::roles::install_route_declarations_for_tests_with;
use oxy_app_core::pods::role_manifest::{RouteRole, classify};

const PARTNER: &str = "33333333-3333-3333-3333-333333333333";

/// Installed the way `oxy-server` does, then asked through the same
/// `classify` the request path uses — so a wrong prefix fails here rather
/// than silently falling to the FleetOk default at runtime. The registry is a
/// process-wide once-cell; nextest gives each test its own process.
#[test]
fn every_partner_org_route_is_served_by_the_fleet() {
    let extra = route_roles()
        .iter()
        .map(|d| (d.method, d.path.to_string(), d.role))
        .collect();
    install_route_declarations_for_tests_with(extra);

    let orgs = format!("/api/partners/{PARTNER}/orgs");
    for (method, path) in [
        ("POST", orgs.clone()),
        ("GET", orgs.clone()),
        ("PATCH", format!("{orgs}/{PARTNER}")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} writes only Postgres"
        );
    }
}
