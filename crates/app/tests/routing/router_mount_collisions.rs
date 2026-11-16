//! Two mounts of one path panic when the router is BUILT, not when it is served.
//!
//! This matters because of how routes are mounted now. `RoleRouter` finishes each
//! route's handlers with their own state and attaches them with `route_service`,
//! and `route_service` does not merge verbs the way `route` does — so the twelve
//! paths that used to be registered twice had to become one call each. A missed
//! one would take the server down at startup, and no unit test asserts "these
//! paths are distinct".
//!
//! Nothing needs to: building the router IS the check. These tests pin the two
//! halves of that reasoning — that a collision really panics, in all three
//! composition forms, and that the real router builds anyway.
//!
//! Each `should_panic` names the message. A bare one passes on ANY panic, so it
//! would go green if axum started rejecting these routers for some unrelated
//! reason — a check that inspects nothing, which is the failure this file exists
//! to rule out.

use axum::routing::{get, post};

#[test]
#[should_panic(expected = "conflict with previously registered route")]
fn route_service_rejects_a_duplicate_path() {
    let _ = axum::Router::<()>::new()
        .route_service("/x", get(|| async { "a" }))
        .route_service("/x", post(|| async { "b" }));
}

/// The contrast, and the reason the conversion needed those twelve merges:
/// `route` accepts the same path twice and unions the verbs.
#[test]
fn route_merges_disjoint_verbs_on_one_path() {
    let _ = axum::Router::<()>::new()
        .route("/x", get(|| async { "a" }))
        .route("/x", post(|| async { "b" }));
}

/// `build_git_routes()` is merged into the workspace tree at the ROOT, with no
/// prefix, so a per-builder scan cannot see a collision between it and a root
/// route. The build can.
#[test]
#[should_panic(expected = "conflict with previously registered route")]
fn merge_rejects_a_colliding_path() {
    let a = axum::Router::<()>::new().route_service("/x", get(|| async { "a" }));
    let b = axum::Router::<()>::new().route_service("/x", post(|| async { "b" }));
    let _ = a.merge(b);
}

#[test]
#[should_panic(expected = "conflict with previously registered route")]
fn nest_rejects_a_path_a_route_already_owns() {
    let inner = axum::Router::<()>::new().route_service("/", get(|| async { "i" }));
    let _ = axum::Router::<()>::new()
        .route_service("/x", get(|| async { "a" }))
        .nest("/x", inner);
}

/// The other half: the real tree, both modes, actually builds. `route_declarations`
/// constructs the cloud router (git features on) and the local one (local setup
/// on), which between them mount every route the server can serve.
#[test]
fn the_real_router_has_no_colliding_mounts() {
    let declared = oxy_app::server::router::route_declarations();
    // 187 on 2026-11-15 (read off this assertion, both modes counted), after
    // the workspace's databases and secrets left for
    // `oxy-api-workspace-config`, the automation listing and the schedules for
    // `oxy-api-automations` and the camera list and the weather for
    // `oxy-api-world-model`; 185 the same day (read off it again; 186 between
    // the two) after the GitHub and Toast webhook receivers left the public
    // tree for `oxy-api-github` and `oxy-api-world-model`, one declaration
    // each; 168 on 2026-11-16 (read off it with the floor raised) after the
    // core routes' Wave A: the tenancy leaves, the Looker listing, the staging
    // compile, `/_catalog` and the four health probes, counted in both modes
    // where they were mounted in both; 148 the same day (read off it) after
    // Wave B's ten conversation routes left the workspace tree for
    // `oxy-api-threads`, counted in both modes, floor `> 150` -> `> 120`; 143
    // (read off it with the floor raised) when org chat's five routes left the
    // cloud-only global tree for the same crate. 104 the same day (read off
    // it) after Wave D2's thirteen semantic routes left the workspace tree for
    // `oxy-api-semantic` (both modes) and its three left the external tree,
    // floor `> 120` -> `> 80`.
    // Surfaces declare their own routes, so this drops
    // with each one that leaves; the floor guards against a tree that stopped
    // building, not a count.
    assert!(
        declared.len() > 80,
        "only {} routes declared — the builder shape changed and this guard \
         stopped covering the tree",
        declared.len()
    );
}
