//! Moved from `oxy-app`'s `server::previews::read_only_tests`: it checks the
//! preview guard's two lists against the generated route table.

use oxy_app::server::previews::read_only::{ALLOWED, REFUSED};
use oxy_app::server::role_manifest::pattern_matches;

use crate::catalog;

/// Relative to the workspace root, as the middleware sees it. The previews API
/// itself is mounted beside the workspace tree, outside `workspace_middleware`,
/// so the guard never sees it.
fn workspace_relative(path: &str) -> Option<&str> {
    path.strip_prefix("/api/{workspace_id}")
        .or_else(|| path.strip_prefix("/external/api/{workspace_id}"))
        .filter(|p| !p.starts_with("/previews"))
}

fn is_mutating(method: &str) -> bool {
    !matches!(method, "GET" | "HEAD" | "OPTIONS")
}

/// Every mutating route the workspace middleware guards is on one of the two
/// lists — so a new one is a decision, not a default — and every entry on the
/// lists names a route that exists, so the lists cannot rot into fiction.
#[test]
fn every_mutating_route_is_classified_and_every_entry_is_real() {
    let routes: Vec<(&str, &str)> = catalog()
        .routes
        .iter()
        .filter(|r| is_mutating(r.method))
        .filter_map(|r| workspace_relative(r.path).map(|p| (r.method, p)))
        .collect();
    assert!(
        routes.len() > 100,
        "the catalog should list the workspace surface ({} found)",
        routes.len()
    );

    let listed = |method: &str, path: &str| {
        let m = if method == "ANY" { "POST" } else { method };
        (m == "POST" && path == "/analytics/runs")
            || ALLOWED
                .iter()
                .any(|(am, p)| *am == m && pattern_matches(p, path))
            || REFUSED
                .iter()
                .any(|(rm, p, _)| (*rm == "*" || *rm == m) && pattern_matches(p, path))
    };
    let unclassified: Vec<String> = routes
        .iter()
        .filter(|(m, p)| !listed(m, p))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    assert!(
        unclassified.is_empty(),
        "these mutating routes are on neither preview list — put each on ALLOWED \
         (what it executes goes through `request_hold`, so its writes are held) or \
         REFUSED (it changes something the execution layer cannot hold):\n  {}",
        unclassified.join("\n  ")
    );

    let hits = |method: &str, pattern: &str| {
        routes.iter().any(|(m, p)| {
            (method == "*" || *m == method || *m == "ANY") && pattern_matches(pattern, p)
        })
    };
    let stale: Vec<String> = ALLOWED
        .iter()
        .map(|(m, p)| (*m, *p))
        .chain(REFUSED.iter().map(|(m, p, _)| (*m, *p)))
        .filter(|(m, p)| !hits(m, p))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    assert!(
        stale.is_empty(),
        "these preview-list entries match no mounted route:\n  {}",
        stale.join("\n  ")
    );
}
