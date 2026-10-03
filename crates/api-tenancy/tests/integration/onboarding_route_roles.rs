//! Every route this crate mounts touches a working copy, so every one must be
//! IdeOnly — asserted here rather than in `oxy-app`'s `global_route_roles.rs`,
//! which is where these three used to live.
//!
//! They moved because the routes did. `oxy-app` cannot depend on this crate
//! (the dependency runs the other way), so its test harness builds a
//! declaration set that does not contain them: `classify` would answer with the
//! FleetOk default and the assertion would fail for the wrong reason. The guard
//! belongs where both halves are visible, which is here.
//!
//! Neither automatic mechanism reaches these. `route_role_derivation` parses
//! `oxy-app`'s `router/workspace.rs` and nothing else. The type gate does not
//! see them either: `setup_demo` clones and scaffolds through the onboarding
//! builder and raw `std::fs` rather than taking a working-copy extractor, so
//! mounting it on the fleet door compiles clean.

use oxy_api_tenancy::onboarding::{route_roles, workspace_route_roles};
use oxy_app::surface::roles::{RouteRole, classify, install_route_declarations_for_tests_with};

const ORG: &str = "11111111-1111-1111-1111-111111111111";
const WORKSPACE: &str = "22222222-2222-2222-2222-222222222222";

/// Install this crate's declarations the way `oxy-server` does, then ask the
/// same `classify` the request path asks at runtime.
fn install() {
    let mut extra: Vec<(&'static str, String, RouteRole)> = route_roles()
        .iter()
        .map(|d| (d.method, d.path.to_string(), d.role))
        .collect();
    // The workspace half is merged INSIDE the `/{workspace_id}` nest, so its
    // declared paths are relative to it — the seam joins the prefix.
    extra.extend(
        workspace_route_roles()
            .iter()
            .map(|d| (d.method, format!("/{{workspace_id}}{}", d.path), d.role)),
    );
    install_route_declarations_for_tests_with(extra);
}

#[test]
fn every_org_scoped_route_reaches_the_ide() {
    install();
    for (method, path) in [
        ("POST", format!("/api/orgs/{ORG}/onboarding/demo")),
        ("POST", format!("/api/orgs/{ORG}/onboarding/new")),
        ("POST", format!("/api/orgs/{ORG}/onboarding/github")),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::IdeOnly,
            "{method} {path} clones a repository and scaffolds config.yml onto \
             node-local disk; on a replica it would clone into a checkout the \
             process does not own",
        );
    }
}

#[test]
fn every_workspace_scoped_route_reaches_the_ide() {
    install();
    for (method, path) in [
        ("GET", format!("/api/{WORKSPACE}/onboarding-readiness")),
        ("GET", format!("/api/{WORKSPACE}/onboarding/github-setup")),
        ("POST", format!("/api/{WORKSPACE}/onboarding/reset")),
        ("POST", format!("/api/{WORKSPACE}/onboarding/test-llm-key")),
        (
            "POST",
            format!("/api/{WORKSPACE}/onboarding/upload-warehouse-files"),
        ),
    ] {
        assert_eq!(
            classify(method, &path),
            RouteRole::IdeOnly,
            "{method} {path} reads or writes the workspace working copy",
        );
    }
}

/// The declaration list is the only thing naming these routes, so a route added
/// to `lib.rs` without a matching entry is invisible to the classifier — and
/// invisible means FleetOk. Count them rather than trusting the eye.
#[test]
fn the_declaration_list_covers_every_mounted_route() {
    assert_eq!(route_roles().len(), 3, "org-scoped routes changed");
    assert_eq!(
        workspace_route_roles().len(),
        5,
        "workspace-scoped routes changed",
    );
    assert!(
        route_roles()
            .iter()
            .chain(workspace_route_roles())
            .all(|d| d.role == RouteRole::IdeOnly),
        "this crate mounts nothing a stateless replica can serve",
    );
}
