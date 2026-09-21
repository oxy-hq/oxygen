//! The routes outside the workspace tree that need the ide, asserted by hand.
//!
//! These five were the last entries in `role_manifest`'s path table. Deleting
//! the table moved them onto their mounts, which is where they belong — but it
//! also removed the only thing that named them, and nothing replaced it:
//!
//!   - `route_role_derivation` parses `router/workspace.rs` and nothing else, so
//!     every route in `router/global.rs` is outside its coverage entirely.
//!   - The type-level gate does not reach them. `setup_demo` builds a workspace
//!     through the onboarding builder rather than taking
//!     `WorkspaceManagerWorkingCopy`, so `route_fleet(.., post(setup_demo))`
//!     COMPILES. Measured: flipping `/onboarding/demo` to the fleet door
//!     produces no compile error and no failing test.
//!
//! So this file is the guard. It is hand-written because the two automatic
//! mechanisms structurally cannot see these routes — not because nobody got
//! around to deriving it.

use oxy_app::server::role_manifest::{RouteRole, classify, install_route_declarations_for_tests};

const ORG: &str = "11111111-1111-1111-1111-111111111111";
const WORKSPACE: &str = "22222222-2222-2222-2222-222222222222";

#[test]
fn org_routes_that_write_a_workspace_reach_the_ide() {
    install_route_declarations_for_tests();

    // Each of these creates or deletes a workspace on disk: the onboarding
    // builder clones and writes a working copy, and the delete removes one.
    // The three `/onboarding/*` creators moved to the `oxy-api-onboarding`
    // sibling crate, and their guard moved with them
    // (`crates/api-onboarding/tests/route_roles.rs`). They cannot be asserted
    // here: `oxy-app` does not depend on that crate, so the declaration set this
    // helper installs does not contain them and `classify` would answer with the
    // FleetOk default — a failure that says nothing about the product.
    let cases: &[(&str, String)] = &[("DELETE", format!("/api/orgs/{ORG}/workspaces/{WORKSPACE}"))];

    for (method, path) in cases {
        assert_eq!(
            classify(method, path),
            RouteRole::IdeOnly,
            "{method} {path} writes a workspace working copy",
        );
    }
}

/// The org surface's reads must NOT follow them to the singleton — listing
/// workspaces is a Postgres query, and pinning it would put the workspace
/// picker behind the ide.
#[test]
fn org_reads_stay_on_the_fleet() {
    install_route_declarations_for_tests();

    for (method, path) in [
        ("GET", format!("/api/orgs/{ORG}/workspaces")),
        ("GET", format!("/api/orgs/{ORG}/members")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} reads Postgres",
        );
    }
}

/// One mount, two pods. `serve_dispatch` answers everything under
/// `/customer-apps/{*path}`: bundle bytes from S3, which any replica serves, and
/// `POST .../fn/<name>`, which executes an Oxy Function against the working
/// copy. `custom_apps_serve::serve_dispatch_roles()` states the split; assert
/// both halves, because a declaration that covered only one would look right.
#[test]
fn a_custom_app_function_runs_on_the_ide_and_its_bundle_does_not() {
    install_route_declarations_for_tests();

    assert_eq!(
        classify("POST", "/customer-apps/acme/dash/fn/send-report"),
        RouteRole::IdeOnly,
        "an Oxy Function executes against the working copy",
    );
    assert_eq!(
        classify("GET", "/customer-apps/acme/dash/index.html"),
        RouteRole::FleetOk,
        "bundle bytes come from S3",
    );
}

/// The document library reads on the fleet; only the answer needs the ide.
///
/// Belongs in this hand-written file for the same reason the rest of it does:
/// `documents::ask::ask` takes no `WorkspaceManagerWorkingCopy` — it builds a
/// project context from the org's workspace row — so the type-level gate
/// cannot see it, and `route_fleet(.., post(ask))` would compile. Measured:
/// flipping the mount produces no compile error.
///
/// The second half is the one that matters more. Answering was first written
/// as `?ask=true` on `/documents/search`, which would have dragged every search
/// in the product onto the singleton — a read that dies when the ide restarts
/// is the HA bug the split fleet exists to prevent. Assert both directions, so
/// a later merge of the two routes fails here rather than in production.
#[test]
fn asking_the_library_reaches_the_ide_and_searching_it_does_not() {
    install_route_declarations_for_tests();

    assert_eq!(
        classify("POST", "/api/documents/ask"),
        RouteRole::IdeOnly,
        "writing an answer resolves an agent config from the working copy",
    );
    assert_eq!(
        classify("GET", "/api/documents/search"),
        RouteRole::FleetOk,
        "searching reads Postgres, and must survive the ide restarting",
    );
}

/// The session store sits one segment under an `IdeOnly` route and is not it.
///
/// `/documents/ask` is `IdeOnly` and `/documents/ask/sessions` is `FleetOk`,
/// which is only true if the manifest matches on the whole path rather than on
/// a prefix. Worth its own test because the failure is invisible: a prefix
/// match would classify the session list `IdeOnly`, every replica would proxy
/// it to the singleton, and the only symptom would be that looking at your own
/// past questions stops working whenever the ide restarts — which is the exact
/// HA bug the split exists to prevent, arrived at through a routing accident
/// rather than a decision.
#[test]
fn the_session_store_stays_on_the_fleet_under_an_ide_only_route() {
    install_route_declarations_for_tests();

    let session = "33333333-3333-3333-3333-333333333333";
    for (method, path) in [
        ("GET", "/api/documents/ask/sessions".to_string()),
        ("POST", "/api/documents/ask/sessions".to_string()),
        ("GET", format!("/api/documents/ask/sessions/{session}")),
        ("DELETE", format!("/api/documents/ask/sessions/{session}")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} reads and writes Postgres only",
        );
    }

    assert_eq!(
        classify("POST", "/api/documents/ask"),
        RouteRole::IdeOnly,
        "and the route they sit under is still pinned",
    );
}

/// Every kiosk route answers from any replica, the new PATCH included.
///
/// A store fixes a counter tablet's sign-out during service. Classifying that
/// write `IdeOnly` would put it behind the singleton — a self-routing proxy hop
/// on a good day, a 421 while the ide restarts on a bad one — for a statement
/// that touches one Postgres row and no working copy at all.
///
/// Asserted here rather than left to the mount for the reason this whole file
/// exists: `route_role_derivation` reads `router/workspace.rs` only, and the
/// type-level gate cannot see a handler that takes no working copy, so
/// `route_ide(.., patch(update_device))` would compile and no test would care.
/// The siblings are listed beside it so a future mount that drags the tree onto
/// the ide fails on all five rather than on whichever one someone remembered.
#[test]
fn changing_a_kiosk_stays_on_the_fleet_like_the_rest_of_them() {
    install_route_declarations_for_tests();

    const DEVICE: &str = "33333333-3333-3333-3333-333333333333";
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
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} reads and writes one Postgres row",
        );
    }
}
