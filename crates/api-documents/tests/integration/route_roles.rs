//! The document library reads on the fleet; only the answer needs the ide.
//!
//! Moved here from `oxy-app`'s `tests/routing/global_route_roles.rs` with the
//! routes. `oxy-app` cannot depend on this crate, so the declaration set its
//! harness installs no longer contains `/documents/ask`: `classify` would answer
//! with the FleetOk default and the IdeOnly assertion would fail for the wrong
//! reason. The guard belongs where both halves are visible, which is here.
//!
//! Hand-written for the same reason the file it came from is: `ask::ask` takes
//! no `WorkspaceManagerWorkingCopy` — it builds a project context from the org's
//! workspace row — so no type gate can see it, and declaring it FleetOk would
//! compile.
//!
//! Only one test installs: the declaration registry is a process-wide
//! once-cell, installed once per test process.

use oxy_api_documents::route_roles;
use oxy_app::server::role_manifest::{
    RouteRole, classify, install_route_declarations_for_tests_with,
};

const ORG: &str = "11111111-1111-1111-1111-111111111111";
const DOC: &str = "22222222-2222-2222-2222-222222222222";
const SESSION: &str = "33333333-3333-3333-3333-333333333333";

/// Install this crate's declarations the way `oxy-server` does, then ask the
/// same `classify` the request path asks at runtime.
fn install() {
    let extra = route_roles()
        .iter()
        .map(|d| (d.method, d.path.to_string(), d.role))
        .collect();
    install_route_declarations_for_tests_with(extra);
}

/// Answering was first written as `?ask=true` on `/documents/search`, which
/// would have dragged every search in the product onto the singleton — a read
/// that dies when the ide restarts is the HA bug the split fleet exists to
/// prevent. Assert both directions, so a later merge of the two routes fails
/// here rather than in production.
///
/// The session store sits one segment under the `IdeOnly` route and is not it,
/// which is only true if the manifest matches on the whole path rather than a
/// prefix. A prefix match would proxy the session list to the singleton, and
/// looking at your own past questions would stop working whenever the ide
/// restarts.
#[test]
fn asking_reaches_the_ide_and_every_other_document_route_does_not() {
    install();

    assert_eq!(
        classify("POST", "/api/documents/ask"),
        RouteRole::IdeOnly,
        "writing an answer resolves an agent config from the working copy",
    );

    for (method, path) in [
        ("GET", "/api/documents/search".to_string()),
        ("GET", "/api/documents".to_string()),
        ("GET", format!("/api/documents/{DOC}")),
        ("GET", format!("/api/documents/{DOC}/download")),
        ("POST", format!("/api/documents/{DOC}/favorite")),
        ("GET", "/api/documents/ask/sessions".to_string()),
        ("POST", "/api/documents/ask/sessions".to_string()),
        ("GET", format!("/api/documents/ask/sessions/{SESSION}")),
        ("DELETE", format!("/api/documents/ask/sessions/{SESSION}")),
        ("POST", format!("/api/orgs/{ORG}/documents")),
        ("PATCH", format!("/api/orgs/{ORG}/documents/{DOC}")),
        ("POST", format!("/api/orgs/{ORG}/documents/{DOC}/versions")),
        ("POST", format!("/api/orgs/{ORG}/document-folders")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::FleetOk,
            "{method} {path} reads and writes Postgres (and presigns S3) only",
        );
    }
}

/// The declaration list is the only thing naming the ide route, so dropping it
/// silently sends `ask` to a replica with no working copy. Count it.
#[test]
fn the_declaration_list_names_only_the_ask_route() {
    let decls = route_roles();
    assert_eq!(decls.len(), 1, "document route declarations changed");
    assert_eq!(decls[0].path, "/documents/ask");
    assert_eq!(decls[0].role, RouteRole::IdeOnly);
}
