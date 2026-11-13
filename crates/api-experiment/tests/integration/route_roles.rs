//! Experiments and simulations read and enqueue on the fleet: every route is
//! FleetOk. (Write-back apply, the one route that needed the ide, was removed
//! with the IDE.)
//!
//! Inside `oxy-app` the routes went through `route_fleet` / `route_ide`, whose
//! types proved the role; a seam router is a plain `Router<AppState>`, so the
//! IdeOnly fact travels as a declaration (`workspace_route_roles`) and this
//! test holds the CLASSIFICATION: what `classify` answers for each path. It
//! never reads a handler body. The other half of what the types proved — that
//! no FleetOk handler reaches for the working copy — is held by the source scan
//! in `crates/app/tests/routing/undeclared_mounts_stay_diskless.rs`, which
//! covers this crate. `oxy-app` cannot depend on this crate, so the
//! declaration set its own harness installs no longer contains the route:
//! `classify` would answer with the FleetOk default and the assertion would
//! fail for the wrong reason. The guard belongs where both halves are visible.
//!
//! Only one test installs: the declaration registry is a process-wide
//! once-cell, installed once per test process.

use oxy_api_experiment::workspace_route_roles;
use oxy_app::surface::roles::{RouteRole, classify, install_route_declarations_for_tests_with};

const WS: &str = "11111111-1111-1111-1111-111111111111";

/// Install this crate's declarations the way `oxy-server` does for the
/// workspace seam — under the `/{workspace_id}` nest — then ask the same
/// `classify` the request path asks at runtime.
fn install() {
    let extra = workspace_route_roles()
        .iter()
        .map(|d| (d.method, format!("/{{workspace_id}}{}", d.path), d.role))
        .collect();
    install_route_declarations_for_tests_with(extra);
}

/// The designs, the price, the run, the sealed result, the worlds and their
/// runs are Postgres, the compile boundary or an enqueue, and a read that dies
/// when one pod restarts is the HA bug the split fleet exists to prevent. A
/// declaration added to one of these paths fails here rather than in
/// production. A handler that grows a working-copy read on a FleetOk route is
/// not something this test can see; the diskless scan named above is.
#[test]
fn every_route_serves_from_any_replica() {
    install();

    for (method, path) in [
        ("GET", format!("/api/{WS}/experiments")),
        ("POST", format!("/api/{WS}/experiments")),
        ("GET", format!("/api/{WS}/experiments/promo_pilot/design")),
        ("PUT", format!("/api/{WS}/experiments/promo_pilot/design")),
        (
            "POST",
            format!("/api/{WS}/experiments/promo_pilot/register"),
        ),
        ("POST", format!("/api/{WS}/experiments/promo_pilot/price")),
        (
            "GET",
            format!("/api/{WS}/experiments/promo_pilot/proposals"),
        ),
        ("GET", format!("/api/{WS}/experiments/promo_pilot/run")),
        ("POST", format!("/api/{WS}/experiments/promo_pilot/abort")),
        ("GET", format!("/api/{WS}/experiments/promo_pilot/result")),
        ("GET", format!("/api/{WS}/experiments/promo_pilot/interim")),
        ("POST", format!("/api/{WS}/experiments/promo_pilot/interim")),
        ("GET", format!("/api/{WS}/simulations")),
        ("POST", format!("/api/{WS}/simulations/validate")),
        ("GET", format!("/api/{WS}/simulations/runs")),
        ("POST", format!("/api/{WS}/simulations/flat/runs")),
        ("GET", format!("/api/{WS}/simulations/flat/race")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} must serve from every replica",
        );
    }
}
