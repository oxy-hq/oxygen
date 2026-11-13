//! The whole document library, answering included, is served by the fleet.
//!
//! Moved here from `oxy-app`'s `tests/routing/global_route_roles.rs` with the
//! routes. `oxy-app` cannot depend on this crate, so the declaration set its
//! harness installs does not contain this crate's routes. The guard belongs
//! where the declarations are visible, which is here.
//!
//! Only one test installs: the declaration registry is a process-wide
//! once-cell, installed once per test process.

use oxy_api_documents::route_roles;
use oxy_app::surface::roles::{RouteRole, classify, install_route_declarations_for_tests_with};

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

/// Answering was first written as `?ask=true` on `/documents/search`, and then
/// pinned to the ide because it read its agent off the working copy. It reads
/// the compiled revision now, so every document route, `ask` included, is served
/// by whichever replica takes it: a read that dies when one pod restarts is the
/// HA bug the split fleet exists to prevent.
#[test]
fn every_document_route_is_served_by_the_fleet() {
    install();
    for (method, path) in [
        ("POST", "/api/documents/ask".to_string()),
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
            "{method} {path}"
        );
    }
}

/// Nothing here needs a Factory, so nothing is declared.
#[test]
fn the_declaration_list_is_empty() {
    assert!(route_roles().is_empty(), "a document route was declared");
}
