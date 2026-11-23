//! `describe_reports_the_fleet_role`, up from `oxy-route-catalog`'s unit tests
//! in decompose RT (2026-11-23). It installs the real router's role
//! declarations (`oxy-app`'s `install_route_declarations_for_tests_with`), so
//! `threads` is read back from the router's own FleetOk declaration rather than
//! from `classify`'s default, and only `oxy-server` depends on both the router
//! and the generated table. nextest runs each test in its own process, so the
//! process-wide list it installs is its own. `routes()`/`describe()` were
//! one-line wrappers over `catalog()` there; they are inlined here.

use oxy_route_catalog::catalog;

#[test]
fn describe_reports_the_fleet_role() {
    // `describe` asks `role_manifest::classify`, which reads the
    // declarations the routers install at startup. A unit test builds no
    // server, so without this the registry is empty and every route reports
    // the FleetOk default — the assertion below would fail for the one
    // reason that says nothing about the catalog.
    //
    // No route needs the ide any more (the GitHub import was the last), so to
    // see `describe` report one this test declares a real catalog route
    // IdeOnly itself; the label is what is under test, not the route.
    oxy_server::role_manifest::install_route_declarations_for_tests_with(vec![(
        "POST",
        "/orgs/{org_id}/onboarding/github".to_string(),
        oxy_server::role_manifest::RouteRole::IdeOnly,
    )]);
    let import = catalog()
        .routes
        .iter()
        .find(|r| r.method == "POST" && r.path == "/api/orgs/{org_id}/onboarding/github")
        .expect("GitHub import route");
    assert_eq!(catalog().describe(import).role, "ide-only");

    let threads = catalog()
        .routes
        .iter()
        .find(|r| r.method == "GET" && r.path == "/api/{workspace_id}/threads")
        .expect("threads listing route");
    assert_eq!(catalog().describe(threads).role, "fleet-ok");
}
