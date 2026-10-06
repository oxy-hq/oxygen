//! Completeness tests for the generated table — moved with it from
//! `oxy-app`'s `server::route_catalog`.

use std::collections::HashSet;

use oxy_app::server::route_catalog::{GeneratedRoute, RouteDescription, searchable_fields};

use super::{GENERATED_SCANNED_DIRS, catalog};

mod app_grant_scope;
mod handler;
mod previews_read_only;
mod sandbox_agent_refusal;
mod sandbox_agent_stack;
mod token_grant_scope;

fn routes() -> &'static [GeneratedRoute] {
    catalog().routes
}

fn surfaces() -> &'static [(&'static str, &'static str, &'static str)] {
    catalog().surfaces
}

fn search(needle: Option<&str>) -> Vec<&'static GeneratedRoute> {
    catalog().search(needle)
}

fn describe(route: &'static GeneratedRoute) -> RouteDescription {
    catalog().describe(route)
}

/// Route groups that must survive any router refactor.
///
/// The bare count floor below cannot see a *shaped* loss: drop every route
/// under one nested builder and the total barely moves. This list is the
/// guard for that — one fragment per subtree the walker has to keep
/// reaching. It is deliberately about groups, not individual routes, so
/// adding or renaming an endpoint does not churn it.
///
/// A crate that mounts at **N** seams needs **N** entries, not one: a group
/// another builder already satisfies pins nothing. `oxy-api-tenancy`
/// fills two seams, and `/api/orgs` alone is satisfied by
/// `build_global_routes` — so without the org entry below, dropping its
/// org seed would lose eight endpoints with every guard still green.
///
/// (A full golden file would catch more, at the cost of a regenerate step
/// in every routing PR. If that trade ever looks right, this is the thing
/// to replace.)
const REQUIRED_GROUPS: &[&str] = &[
    "/api/health",
    "/api/auth/",
    "/api/orgs",
    // The org seam of `oxy-api-tenancy`'s onboarding module; `/api/orgs` above does not
    // cover it (see the note on N seams).
    "/api/orgs/{org_id}/onboarding",
    // `oxy-api-documents`: one seam, two subtrees — the reads at the root
    // and the writes under the org, which `/api/orgs` above also cannot see.
    "/api/documents",
    "/api/orgs/{org_id}/documents",
    // Both seams of `oxy-api-frontline`: the org-admin routes, and the
    // PIN sign-in on the public seam.
    "/api/orgs/{org_id}/frontline",
    "/api/frontline/",
    "/api/customer-apps",
    "/api/assume",
    "/api/airhouse/",
    "/api/partners/",
    "/api/user/github/",
    "/api/admin/apps",
    "/api/admin/compiles",
    "/api/admin/internal-jobs",
    "/api/admin/feature-flags",
    "/api/control/",
    "/api/fleet/",
    "/api/{workspace_id}/threads",
    "/api/{workspace_id}/agents",
    "/api/{workspace_id}/files",
    "/api/{workspace_id}/databases",
    "/api/{workspace_id}/secrets",
    "/api/{workspace_id}/apps",
    "/api/{workspace_id}/api-keys",
    "/api/{workspace_id}/tests",
    "/api/{workspace_id}/traces",
    "/api/{workspace_id}/metrics",
    "/api/{workspace_id}/execution-analytics",
    "/api/{workspace_id}/semantic",
    "/api/{workspace_id}/analytics",
    "/api/{workspace_id}/agentic-workflows",
    "/api/{workspace_id}/agentic-airway",
    "/api/{workspace_id}/agentic-schedules",
    "/api/{workspace_id}/world-model",
    "/api/{workspace_id}/sql/",
    "/api/{workspace_id}/integrations",
    "/api/{workspace_id}/repositories",
    "/api/{workspace_id}/onboarding",
    // `oxy-api-source-upload`, on the workspace seam.
    "/api/{workspace_id}/source-uploads",
    "/api/{workspace_id}/cameras",
    "/external/api/",
];

/// Files that mount routes outside the trees the walker scans, each for a
/// reason. Kept explicit so a *new* one fails
/// [`every_route_tree_is_scanned`] instead of silently going unlisted —
/// which is how a whole sibling API crate would otherwise disappear.
const UNSCANNED_ROUTE_FILES: &[&str] = &[
    // The walker itself: `.route(` appears as a string literal.
    "crates/route-catalog/build_route_catalog.rs",
    // Mounts `/customer-apps/{*path}` and the SwaggerUI tree at the top
    // level, outside `/api` — deliberately out of scope (see module docs).
    "crates/app/src/cli/commands/serve.rs",
    // The `oxy worker --health-port` surface: /healthz, /readyz, /metrics.
    "crates/app/src/server/worker_health.rs",
    // The `OXY_METRICS_PORT` surface on serve/ide: `/metrics`, on its own
    // listener rather than the product router. Not an API route and
    // deliberately not in the catalog — it is scraped in-cluster, carries
    // no `/api` prefix, and is not something an `oxyc` user calls.
    "crates/app/src/server/metrics_server.rs",
    // `oxy-oltp`'s router, merged at the protected-tree root. Wiring it
    // into SOURCE_DIRS needs a seed, and a seed resolves a builder by
    // module-path SUFFIX — this crate's entry point is `api::router`,
    // which collides with `airhouse`'s and `cameras`'s. The walker has no
    // way to say "the one in crates/oltp", so the tree indexes and
    // contributes nothing, which trips `every_scanned_tree_contributes_routes`
    // instead. Listed here rather than half-wired: `oxyc routes` does
    // not show `/api/oltp/*`, and saying so is better than a seed that
    // silently points at the wrong crate.
    "crates/oltp/src/api/mod.rs",
    // Test files, not route trees. They name `route_fleet` / `route_ide` in
    // assertions and fixtures, and the walker learned those markers when the
    // routers moved to `RoleRouter` — so they started looking like mounts.
    "crates/app/src/server/role_manifest_tests.rs",
    "crates/app/src/server/role_middleware_tests.rs",
];

/// The floor exists so a router refactor the lexical walker can no longer
/// follow fails here instead of quietly emptying `oxyc routes`.
#[test]
fn catalog_covers_the_whole_surface() {
    assert!(
        routes().len() > 400,
        "route catalog collapsed to {} entries — build_route_catalog.rs can no longer \
         follow the router source. Check the SEEDS list and the .nest/.merge walk.",
        routes().len()
    );
}

/// Complements the count floor: a nested subtree can vanish whole without
/// moving the total much, but not without emptying one of these.
#[test]
fn every_route_group_survives() {
    for group in REQUIRED_GROUPS {
        assert!(
            routes().iter().any(|r| r.path.starts_with(group)),
            "no routes left under {group:?} — a builder the walk used to \
             reach is no longer being followed"
        );
    }
}

/// The catalog is only as complete as `SOURCE_DIRS`, and adding a tree to
/// that list is a step someone will forget — adding `oxy-api-partner-console`
/// took four separate list edits. This walks the workspace and fails on any
/// file that mounts routes from outside the scanned trees and is not a
/// known, reasoned exception.
///
/// Reads the checkout, so it no-ops where the sources are not present.
#[test]
fn every_route_tree_is_scanned() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let crates = root.join("crates");
    if !crates.is_dir() {
        return;
    }

    let mut unscanned = Vec::new();
    collect_route_files(&crates, &root, &mut unscanned);
    unscanned.retain(|rel| {
        !GENERATED_SCANNED_DIRS
            .iter()
            .any(|(dir, _)| rel.starts_with(&format!("{dir}/")))
            && !UNSCANNED_ROUTE_FILES.contains(&rel.as_str())
    });
    unscanned.sort();

    assert!(
        unscanned.is_empty(),
        "these files mount routes but sit outside every scanned tree, so \
         `oxyc routes` will not list them: {unscanned:#?}\n\
         Add the tree to SOURCE_DIRS in crates/route-catalog/build_route_catalog.rs, \
         or add the file to UNSCANNED_ROUTE_FILES with the reason."
    );

    for known in UNSCANNED_ROUTE_FILES {
        assert!(
            root.join(known).exists(),
            "UNSCANNED_ROUTE_FILES lists {known:?}, which no longer exists — \
             remove the stale entry."
        );
    }
}

/// Every scanned tree has to actually produce routes.
///
/// `every_route_tree_is_scanned` closes one half — a tree that declares
/// routes and is not listed. This closes the other: a tree that *is*
/// listed but is never walked, because whoever added it to `SOURCE_DIRS`
/// forgot the matching `SEEDS` entry. That combination is otherwise
/// completely silent — the crate gets indexed, no seed warning fires
/// (those only report a seed that is listed and no longer resolves), and
/// no count moves.
///
/// Granularity is the *tree*, which is the unit a new sibling crate
/// arrives as. A single builder going unreached inside a tree that still
/// contributes elsewhere is not caught here — `every_route_group_survives`
/// is the guard for that shape.
#[test]
fn every_scanned_tree_contributes_routes() {
    for (dir, contributed) in GENERATED_SCANNED_DIRS {
        assert!(
            *contributed > 0,
            "{dir:?} is in SOURCE_DIRS but produced no routes. Most likely its \
             builder has no entry in SEEDS (crates/route-catalog/build_route_catalog.rs), so \
             the tree is indexed and never walked — remove the tree or wire up the \
             seed. Also possible, if rarer: every route it mounts duplicates \
             another tree's, since the count is taken after `collect`'s dedup."
        );
    }
}

/// Repo-relative paths of every non-test source file containing a
/// `.route(` mount.
///
/// Mirrors two of the walker's skip rules by hand — `tests` directories
/// and the `#[cfg(test)]` truncation. Kept in step with `collect_dir` and
/// `truncate_at_test_module` in `crates/route-catalog/build_route_catalog.rs`: if
/// either changes, change this too, or the test starts reporting files the
/// walker would never have looked at.
fn collect_route_files(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "tests" && name != "target" && name != "node_modules" {
                collect_route_files(&path, root, out);
            }
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // `#[cfg(test)]` modules build probe routers that never ship.
        let src = text.split("\n#[cfg(test)]").next().unwrap_or_default();
        if src.contains(".route(")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

#[test]
fn every_surface_is_reachable() {
    let found: HashSet<&str> = routes().iter().map(|r| r.surface).collect();
    for (surface, _, _) in surfaces() {
        assert!(
            found.contains(surface),
            "no routes found for the {surface:?} surface — its seed in \
             build_route_catalog.rs no longer resolves"
        );
    }
}

/// One landmark per surface. These are load-bearing endpoints; if the walk
/// stops reaching one, the help is lying to whoever reads it.
#[test]
fn landmark_routes_are_present() {
    for (method, path) in [
        ("GET", "/api/health"),
        ("GET", "/api/user"),
        ("GET", "/api/orgs"),
        ("GET", "/api/orgs/{org_id}/workspaces"),
        ("GET", "/api/admin/apps"),
        ("GET", "/api/{workspace_id}/threads"),
        ("GET", "/api/{workspace_id}/agents"),
        ("POST", "/api/{workspace_id}/sql/query"),
        ("POST", "/api/{workspace_id}/semantic"),
        ("POST", "/external/api/{workspace_id}/sql/query"),
    ] {
        assert!(
            routes()
                .iter()
                .any(|r| r.method == method && r.path == path),
            "landmark route {method} {path} missing from the catalog"
        );
    }
}

/// The previews tree is mounted beside the workspace tree, reached by its
/// own seed in `build_route_catalog.rs`; its fleet routes must be listed
/// too, or `oxyc routes previews` hides the checks endpoint.
#[test]
fn the_previews_checks_route_is_catalogued() {
    assert!(
        routes()
            .iter()
            .any(|r| r.method == "GET" && r.path == "/api/{workspace_id}/previews/checks"),
        "GET /api/{{workspace_id}}/previews/checks missing from the catalog"
    );
}

/// The held procedure runs routes, from the same seed.
#[test]
fn the_previews_runs_routes_are_catalogued() {
    for (method, path) in [
        ("POST", "/api/{workspace_id}/previews/runs"),
        ("GET", "/api/{workspace_id}/previews/runs"),
        ("GET", "/api/{workspace_id}/previews/runs/{run_id}"),
    ] {
        assert!(
            routes()
                .iter()
                .any(|r| r.method == method && r.path == path),
            "{method} {path} missing from the catalog"
        );
    }
}

/// Airway samples' sandbox sources, from the same seed.
#[test]
fn the_previews_sources_routes_are_catalogued() {
    for method in ["GET", "PUT"] {
        assert!(
            routes()
                .iter()
                .any(|r| r.method == method && r.path == "/api/{workspace_id}/previews/sources"),
            "{method} /api/{{workspace_id}}/previews/sources missing from the catalog"
        );
    }
}

#[test]
fn paths_are_well_formed() {
    for r in routes() {
        assert!(
            r.path.starts_with("/api/") || r.path.starts_with("/external/api/"),
            "route {} {} is mounted outside the API surface",
            r.method,
            r.path
        );
        assert!(
            !r.path.contains("//"),
            "route {} has an empty path segment",
            r.path
        );
        assert!(
            r.method.chars().all(|c| c.is_ascii_uppercase()),
            "method {:?} should be uppercase",
            r.method
        );
    }
}

#[test]
fn entries_are_unique() {
    let mut seen = HashSet::new();
    for r in routes() {
        assert!(
            seen.insert((r.method, r.path, r.surface)),
            "duplicate catalog entry {} {} ({})",
            r.method,
            r.path,
            r.surface
        );
    }
}

/// The point of harvesting prose is that a caller learns what a route does
/// without the source. If the harvest breaks, the routes are still listed
/// and nothing else fails — so assert on it explicitly.
#[test]
fn a_good_share_of_routes_carry_prose() {
    let documented = routes()
        .iter()
        .filter(|r| !r.description.is_empty() || !r.note.is_empty())
        .count();
    assert!(
        documented * 3 > routes().len(),
        "only {documented} of {} routes carry a description or a note — the doc \
         harvest in build_route_catalog.rs stopped resolving handlers",
        routes().len()
    );
}

#[test]
fn describe_reports_the_surface_credential() {
    let health = routes()
        .iter()
        .find(|r| r.path == "/api/health")
        .expect("health route");
    let described = describe(health);
    assert_eq!(described.surface, "public");
    assert!(described.credential.contains("no credential"));
}

#[test]
fn search_filters_by_any_searchable_field() {
    let hits = search(Some("threads"));
    assert!(!hits.is_empty());
    // ASSERTED THROUGH `searchable_fields`, not against `path`. This test
    // used to require every hit's PATH to contain the needle, which was
    // true only while the filter read three fields. Now that it reads
    // `description` too, a hit whose doc comment says "threads" is correct
    // behaviour — and the old assertion would have called it a defect the
    // first time a handler doc moved.
    for hit in &hits {
        assert!(
            searchable_fields(hit)
                .iter()
                .any(|f| f.to_lowercase().contains("threads")),
            "{} {} was returned but contains the needle in no searchable field",
            hit.method,
            hit.path
        );
    }
    assert_eq!(search(None).len(), routes().len());
}

#[test]
fn describe_reports_the_fleet_role() {
    // `describe` asks `role_manifest::classify`, which reads the
    // declarations the routers install at startup. A unit test builds no
    // server, so without this the registry is empty and every route reports
    // the FleetOk default — the assertion below would fail for the one
    // reason that says nothing about the catalog.
    oxy_app::server::role_manifest::install_route_declarations_for_tests();

    // `/files` reads the working copy, so it is pinned to the ide; the
    // thread list is served from Postgres and runs anywhere.
    let files = routes()
        .iter()
        .find(|r| r.method == "GET" && r.path == "/api/{workspace_id}/files")
        .expect("files listing route");
    assert_eq!(describe(files).role, "ide-only");

    let threads = routes()
        .iter()
        .find(|r| r.method == "GET" && r.path == "/api/{workspace_id}/threads")
        .expect("threads listing route");
    assert_eq!(describe(threads).role, "fleet-ok");
}

/// Every surface the catalog advertises has at least one route on it.
///
/// A surface with no routes is worse than a missing one: `oxyc routes`
/// renders its heading and credential line, so the reader is told the
/// surface exists and shown nothing under it.
#[test]
fn every_advertised_surface_has_routes() {
    for (surface, label, _) in surfaces() {
        assert!(
            routes().iter().any(|r| r.surface == *surface),
            "the {label:?} surface is advertised but has no routes"
        );
    }
    assert!(routes().iter().any(|r| r.path == "/api/health"));
    assert!(
        routes()
            .iter()
            .any(|r| r.path.starts_with("/external/api/"))
    );
}
