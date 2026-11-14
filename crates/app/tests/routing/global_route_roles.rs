//! The roles of routes outside the workspace tree, asserted by hand.
//!
//! None of them needs the ide any more (2026-11-14); the guard stays because the
//! reasons below still hold for any route that comes to need it.
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

/// Deleting a workspace is a Postgres delete (the row, its schedules, its
/// Airhouse tenant record) and stays on the fleet. It used to be pinned to the
/// ide because it also removed the working copy; since step B of the Factory
/// retirement (2026-11-14) it removes files only where the serving process owns
/// them (`process_owns_workspace_files`), so a fleet replica answers it whole.
#[test]
fn deleting_a_workspace_stays_on_the_fleet() {
    install_route_declarations_for_tests();
    let path = format!("/api/orgs/{ORG}/workspaces/{WORKSPACE}");
    assert_eq!(
        classify("DELETE", &path),
        RouteRole::FleetOk,
        "DELETE {path} is Postgres only"
    );
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

// The custom-app serve route (`/customer-apps/{*path}`, bundle bytes and
// `POST …/fn/<name>` both FleetOk) and the health, logs and errors reads moved to
// the `oxy-api-custom-apps` sibling crate, and their guard moved with them
// (`crates/api-custom-apps/tests/integration/route_roles.rs`). Not assertable
// here: the declaration set this harness installs no longer contains them, so
// `classify` would answer with the FleetOk default and pass for the wrong reason.

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
