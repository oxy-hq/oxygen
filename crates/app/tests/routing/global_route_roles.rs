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

// The two document-library cases (`/documents/ask` on the ide, its search and
// session store on the fleet) moved to the `oxy-api-documents` sibling crate with
// the routes: `crates/api-documents/tests/integration/route_roles.rs`. They cannot
// be asserted here — `oxy-app` does not depend on that crate, so the declaration
// set this helper installs does not contain `/documents/ask` and `classify` would
// answer with the FleetOk default.

// The kiosk and worker routes (`/orgs/{org_id}/frontline/*`) and the frontline
// sign-in moved to the `oxy-api-frontline` sibling crate, and their FleetOk guard
// moved with them (`crates/api-frontline/tests/integration/route_roles.rs`). Not
// assertable here for the reason given above for onboarding: the declaration set
// this harness installs no longer contains them, so `classify` would answer with
// the FleetOk default and pass for the wrong reason.
