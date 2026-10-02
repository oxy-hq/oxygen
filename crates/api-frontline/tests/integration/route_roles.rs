//! Every frontline route answers from any replica.
//!
//! Moved here from `oxy-app`'s `tests/routing/global_route_roles.rs` with the
//! routes. `oxy-app` cannot depend on this crate, so the declaration set its
//! harness installs no longer contains them, and `classify` there would answer
//! with the FleetOk default — a pass that says nothing about the product. Here
//! both halves are visible: the declarations are installed the way `oxy-server`
//! installs them, then asked through the same `classify` the request path uses.
//!
//! The kiosk PATCH is the case that motivated the original assertion: a store
//! fixes a counter tablet's sign-out during service, and classifying that write
//! `IdeOnly` would put it behind the singleton — a proxy hop on a good day, a
//! 421 while the ide restarts on a bad one — for one Postgres row. And sign-in
//! pinned to the singleton would lock every store out of its checklists on each
//! deploy.

use oxy_api_frontline::{public_route_roles, route_roles};
use oxy_app::surface::roles::{RouteRole, classify, install_route_declarations_for_tests_with};

const ORG: &str = "11111111-1111-1111-1111-111111111111";
const DEVICE: &str = "33333333-3333-3333-3333-333333333333";
const WORKER: &str = "44444444-4444-4444-4444-444444444444";

/// Both seams merge at the `/api` root with no prefix, so their declared paths
/// are absolute and the installer only adds `/api`.
fn install() {
    let extra = route_roles()
        .iter()
        .chain(public_route_roles())
        .map(|d| (d.method, d.path.to_string(), d.role))
        .collect();
    install_route_declarations_for_tests_with(extra);
}

#[test]
fn changing_a_kiosk_stays_on_the_fleet_like_the_rest_of_them() {
    install();
    for (method, path) in [
        (
            "PATCH",
            format!("/api/orgs/{ORG}/frontline/devices/{DEVICE}"),
        ),
        (
            "DELETE",
            format!("/api/orgs/{ORG}/frontline/devices/{DEVICE}"),
        ),
        ("GET", format!("/api/orgs/{ORG}/frontline/devices")),
        ("POST", format!("/api/orgs/{ORG}/frontline/devices")),
        (
            "POST",
            format!("/api/orgs/{ORG}/frontline/devices/{DEVICE}/enrol-link"),
        ),
        // Leaving kiosk mode from the tablet: a manager freeing a stuck phone
        // must not need the singleton to be up.
        ("POST", format!("/api/orgs/{ORG}/frontline/device/leave")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} reads and writes one Postgres row",
        );
    }
}

#[test]
fn enrolling_and_managing_workers_stays_on_the_fleet() {
    install();
    for (method, path) in [
        ("GET", format!("/api/orgs/{ORG}/frontline/workers")),
        ("POST", format!("/api/orgs/{ORG}/frontline/workers")),
        (
            "PATCH",
            format!("/api/orgs/{ORG}/frontline/workers/{WORKER}"),
        ),
        (
            "PUT",
            format!("/api/orgs/{ORG}/frontline/workers/{WORKER}/apps"),
        ),
        (
            "POST",
            format!("/api/orgs/{ORG}/frontline/workers/{WORKER}/pin"),
        ),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} reads and writes Postgres only",
        );
    }
}

#[test]
fn signing_in_survives_the_ide_restarting() {
    install();
    for (method, path) in [
        ("GET", "/api/frontline/roster"),
        ("POST", "/api/frontline/login"),
        ("GET", "/api/frontline/device"),
        ("GET", "/api/frontline/devices/bind"),
        ("POST", "/api/frontline/devices/bind"),
    ] {
        assert_eq!(
            classify(method, path),
            RouteRole::FleetOk,
            "{method} {path} is the only way a worker gets a session",
        );
    }
}
