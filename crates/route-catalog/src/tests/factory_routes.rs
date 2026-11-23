//! Every workspace route is on one of the two Factory lists — and every entry on
//! them names a route that exists. Moved beside the generated catalog it reads, like
//! `previews_read_only`.
//!
//! A new route under `/api/{workspace_id}/…` must be put on
//! `server::factory_access::routes::{CUSTOMER, FACTORY}` (`oxy_workspace_runtime::request::factory_access`
//! since 2026-11-22, re-exported there): the Factory's, refused to
//! everyone but Oxy staff, the org's owners and admins and the members an operator
//! granted the workspace to once
//! enforcement is on, or a customer's, open to everyone with access. Without a
//! decision the route is treated as the Factory's at runtime; this test is what makes
//! that a decision someone made rather than a default nobody noticed.
//!
//! Not covered, deliberately: the external API (`/external/api/{workspace_id}/…`,
//! a curated customer surface the gate lets through whole), `/previews` (mounted
//! beside the workspace tree, already staff-only), and everything outside
//! `/{workspace_id}` — see the module doc of `routes.rs`.
//!
//! The two tests that compose the served router's declarations
//! (`every_customer_route_is_served_by_a_serve_pod`,
//! `every_exception_is_still_needed`) went up to `oxy-server`'s integration
//! group in decompose RT (2026-11-23), `route_catalog::factory_routes`: they
//! build the real router, which only `oxy-server` can depend on beside this
//! table.

use oxy_app_core::pods::role_manifest::pattern_matches;
use oxy_workspace_runtime::request::factory_access::routes::{
    CUSTOMER, FACTORY, Resolution, Surface, resolve, surface_of,
};

use crate::catalog;

const VERBS: [&str; 5] = ["GET", "POST", "PUT", "PATCH", "DELETE"];

/// `(method, path relative to the workspace root)` for every route the workspace
/// middleware gates on the main API.
fn workspace_routes() -> Vec<(&'static str, &'static str)> {
    catalog()
        .routes
        .iter()
        .filter_map(|r| {
            let rel = r.path.strip_prefix("/api/{workspace_id}")?;
            // Mounted beside the tree, behind `workspace_access_middleware`.
            (!rel.starts_with("/previews")).then_some((r.method, rel))
        })
        .collect()
}

/// The verbs a catalog method stands for. `ANY` is every verb.
fn verbs_of(method: &str) -> Vec<&str> {
    if method == "ANY" {
        VERBS.to_vec()
    } else {
        vec![method]
    }
}

#[test]
fn every_workspace_route_is_on_one_of_the_two_lists() {
    let routes = workspace_routes();
    assert!(
        routes.len() > 250,
        "the catalog should list the workspace surface ({} found)",
        routes.len()
    );

    let mut unlisted = Vec::new();
    let mut ambiguous = Vec::new();
    for (method, path) in &routes {
        for verb in verbs_of(method) {
            match resolve(verb, path) {
                Resolution::Listed(_) => {}
                Resolution::Unlisted => unlisted.push(format!("{verb} {path}")),
                Resolution::Ambiguous => ambiguous.push(format!("{verb} {path}")),
            }
        }
    }
    assert!(
        unlisted.is_empty(),
        "these workspace routes are on neither Factory list — put each on CUSTOMER (a \
         surface a customer reaches calls it) or FACTORY (it authors or operates the \
         workspace) in `crates/workspace-runtime/src/request/factory_access/routes.rs`:\n  {}",
        unlisted.join("\n  ")
    );
    assert!(
        ambiguous.is_empty(),
        "the most specific CUSTOMER and FACTORY entries tie on these routes, so which \
         side wins is not decided — make one more specific:\n  {}",
        ambiguous.join("\n  ")
    );
}

#[test]
fn every_entry_names_a_route_that_exists() {
    let routes = workspace_routes();
    let hits = |method: &str, pattern: &str| {
        routes.iter().any(|(m, p)| {
            (method == "*" || *m == method || *m == "ANY") && pattern_matches(pattern, p)
        })
    };
    let stale: Vec<String> = CUSTOMER
        .iter()
        .map(|e| ("customer", e))
        .chain(FACTORY.iter().map(|e| ("factory", e)))
        .filter(|(_, (method, pattern))| !hits(method, pattern))
        .map(|(list, (method, pattern))| format!("{list}: {method} {pattern}"))
        .collect();
    assert!(
        stale.is_empty(),
        "these Factory-list entries match no mounted workspace route — delete them, or fix \
         the pattern:\n  {}",
        stale.join("\n  ")
    );
}

/// Landmarks: the routes the back office is built from are the Factory's and the ones a
/// customer's surfaces are built from are not. If one of these flips, someone has
/// changed what the Factory is, and should have meant to.
#[test]
fn the_landmarks_sit_where_the_plan_puts_them() {
    for (method, path) in [
        ("POST", "/compile/staging"),
        ("GET", "/semantic/objects"),
        ("GET", "/semantic/topic/{file_path_b64}"),
        ("PUT", "/secrets/{id}"),
        ("POST", "/world-model/competitors"),
        ("POST", "/agentic-airway/backfill"),
        ("POST", "/agentic-schedules"),
    ] {
        assert_eq!(
            surface_of(method, path),
            Surface::Factory,
            "{method} {path}"
        );
    }
    for (method, path) in [
        ("GET", "/meta"),
        ("GET", "/threads"),
        ("POST", "/threads"),
        ("POST", "/analytics/runs"),
        ("GET", "/custom-apps"),
        ("GET", "/databases"),
        ("GET", "/api-tokens"),
        ("GET", "/oxy-access"),
        ("POST", "/sql/query"),
        ("GET", "/world-model/events"),
        ("POST", "/world-model/weather/current"),
    ] {
        assert_eq!(
            surface_of(method, path),
            Surface::Customer,
            "{method} {path}"
        );
    }
}

/// The split, in numbers, for the PR that changes it: run with `--nocapture`.
#[test]
fn report_the_split() {
    let (mut factory, mut customer) = (0, 0);
    for (method, path) in workspace_routes() {
        for verb in verbs_of(method) {
            match surface_of(verb, path) {
                Surface::Factory => factory += 1,
                Surface::Customer => customer += 1,
            }
        }
    }
    println!("factory-routes: {factory} Factory, {customer} customer");
    assert!(factory > 0 && customer > 0);
}
