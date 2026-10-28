//! Every workspace route is on one of the two Factory lists — and every entry on
//! them names a route that exists. Moved beside the generated catalog it reads, like
//! `previews_read_only`.
//!
//! A new route under `/api/{workspace_id}/…` must be put on
//! `server::factory_access::routes::{CUSTOMER, FACTORY}`: the Factory's, refused to
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

use oxy_app::server::factory_access::routes::{
    CUSTOMER, FACTORY, Resolution, Surface, resolve, surface_of,
};
use oxy_app::server::role_manifest::{RouteRole, declared_role_in, pattern_matches};

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
         workspace) in `crates/app/src/server/factory_access/routes.rs`:\n  {}",
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

/// Landmarks: the routes the IDE is built from are the Factory's and the ones a
/// customer's surfaces are built from are not. If one of these flips, someone has
/// changed what the Factory is, and should have meant to.
#[test]
fn the_landmarks_sit_where_the_plan_puts_them() {
    for (method, path) in [
        ("GET", "/files"),
        ("POST", "/files/{pathb64}"),
        ("POST", "/compile"),
        ("POST", "/push-changes"),
        ("GET", "/modeling/{project_name}/lineage"),
        ("GET", "/semantic/topic/{file_path_b64}"),
        ("POST", "/databases"),
        ("PUT", "/secrets/{id}"),
        ("GET", "/world-model/events"),
        ("POST", "/agentic-airway/backfill"),
        ("POST", "/agentic-schedules"),
        ("GET", "/tests"),
        ("GET", "/apps/source/{pathb64}"),
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
        ("GET", "/api-keys"),
        ("GET", "/oxy-access"),
        ("POST", "/sql/query"),
        // A data app's result file: opening an app or a chat artifact needs it.
        ("GET", "/apps/file/{pathb64}"),
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

/// Customer routes a serve pod does not handle yet, each tied to the change
/// that ends it. Exact `(method, path)` as on [`CUSTOMER`].
///
/// - The four Data App routes: Data Apps are removed by #3835, which deletes
///   the routes. Running a Data App reads its `.app.yml` and result files off
///   the working copy.
///
/// (The four shell and launcher reads, `/git-state`, `/status`,
/// `/onboarding-readiness` and `/onboarding/github-setup`, left this list in #3849,
/// which moved them to the Factory list.)
const NOT_ON_SERVE_YET: &[(&str, &str)] = &[
    // Removed with Data Apps (#3835).
    ("GET", "/apps/{pathb64}"),
    ("GET", "/apps/file/{pathb64}"),
    ("POST", "/apps/{pathb64}/result"),
    ("POST", "/apps/{pathb64}/run"),
];

/// Every role declaration a served router installs, composed the way
/// `oxy-server` composes it: `oxy-app`'s own, plus the workspace seam's
/// surfaces joined under the `/{workspace_id}` nest and the `/api` prefix.
///
/// Read through [`declared_role_in`] rather than installed: the process-wide
/// list is set once, and another test in this binary may already have set it
/// without the surfaces.
fn served_declarations() -> Vec<(&'static str, String, RouteRole)> {
    let mut decls = oxy_app::server::router::route_declarations();
    let workspace_seam = oxy_api_tenancy::onboarding::workspace_route_roles()
        .iter()
        .chain(oxy_api_source_upload::route_roles())
        .chain(oxy_api_custom_apps::workspace_route_roles());
    decls.extend(
        workspace_seam.map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role)),
    );
    decls
}

/// The rule "the main instance serving users serves from `oxy serve`": every
/// route a customer reaches is one a serve pod answers itself, so the
/// user-facing product never needs the Factory pod. A route on [`CUSTOMER`]
/// that only the Factory serves is either moved to the fleet or, if it is
/// authoring, moved to [`FACTORY`] — never left for a customer to reach the
/// Factory through.
///
/// Walks the catalog, so a customer route a surface crate mounts is held to
/// the rule as well as `oxy-app`'s.
#[test]
fn every_customer_route_is_served_by_a_serve_pod() {
    let decls = served_declarations();
    let mut on_the_factory = Vec::new();
    let mut undeclared = Vec::new();
    let mut checked = 0;
    for (method, rel) in workspace_routes() {
        for verb in verbs_of(method) {
            if surface_of(verb, rel) != Surface::Customer {
                continue;
            }
            let excepted = NOT_ON_SERVE_YET
                .iter()
                .any(|(m, p)| *m == verb && *p == rel);
            let path = format!("/api/{{workspace_id}}{rel}");
            match declared_role_in(&decls, verb, &path) {
                Some(RouteRole::FleetOk) => checked += 1,
                Some(_) if excepted => {}
                Some(role) => on_the_factory.push(format!("{verb} {rel} ({})", role.as_str())),
                None => undeclared.push(format!("{verb} {rel}")),
            }
        }
    }
    assert!(
        checked > 50,
        "the walk should reach the customer surface ({checked} routes checked)"
    );
    assert!(
        undeclared.is_empty(),
        "these customer routes have no role declaration in the composition this \
         test builds — a surface crate's seam is missing from \
         `served_declarations`:\n  {}",
        undeclared.join("\n  ")
    );
    assert!(
        on_the_factory.is_empty(),
        "these routes are on CUSTOMER but only the Factory pod serves them, so a \
         customer's page needs the Factory. Make the handler read the compiled \
         revision and declare it FleetOk, or, if it authors the workspace, move it \
         to FACTORY in `crates/app/src/server/factory_access/routes.rs`:\n  {}",
        on_the_factory.join("\n  ")
    );
}

/// An exception that no longer excepts anything is deleted, so the list only
/// ever shrinks to empty: each entry must still be on [`CUSTOMER`] and still
/// be served by the Factory alone.
#[test]
fn every_exception_is_still_needed() {
    let decls = served_declarations();
    let mut stale = Vec::new();
    for (method, rel) in NOT_ON_SERVE_YET {
        let on_customer = surface_of(method, rel) == Surface::Customer;
        let path = format!("/api/{{workspace_id}}{rel}");
        let factory_only = matches!(
            declared_role_in(&decls, method, &path),
            Some(role) if role != RouteRole::FleetOk
        );
        if !(on_customer && factory_only) {
            stale.push(format!(
                "{method} {rel} (on CUSTOMER: {on_customer}, Factory only: {factory_only})"
            ));
        }
    }
    assert!(
        stale.is_empty(),
        "delete these from NOT_ON_SERVE_YET — they are no longer a customer route \
         only the Factory serves:\n  {}",
        stale.join("\n  ")
    );
}
