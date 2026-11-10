//! `oxy-app-core` holds what every HTTP surface needs, and nothing a single
//! bounded context owns (rule S9 in `internal-docs/domain-boundaries.md`; the
//! charter is `crates/app-core/CLAUDE.md`).
//!
//! Every crate above this one rebuilds when it changes, so a context parked here
//! costs every surface a rebuild on every edit. This test closes the door the
//! same way `crates/app/tests/routing/app_growth.rs` does for `oxy-app`: the
//! `pub mod`s of `lib.rs` are a closed list.
//!
//! - **`CHARTER`**: modules that pass the admission test, tagged with their kind.
//!   A new module goes here only with a kind a reviewer can check.
//! - **`BACKLOG`**: modules that have an owning context, with the crate they
//!   belong in. **Shrink-only**: never add a row; delete one when its module
//!   moves to its owner.
//!
//! A module in neither list fails, as does a listed module that no longer exists.

use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    /// Request state and context every surface reads.
    Request,
    /// Authorization and refusal plumbing (decisions stay in `oxy-authz`).
    Authz,
    /// A cross-cutting service with no owning context.
    Service,
    /// A trait that lets a lower crate call up without depending up.
    Seam,
}

use Kind::*;

/// Modules that belong here. Sorted.
const CHARTER: &[(&str, Kind)] = &[
    ("agent_body", Authz),
    ("agent_fixture", Authz),
    ("agent_refusal", Authz),
    ("app_state", Request),
    ("audit", Service),
    ("compiled_reader", Service),
    ("config", Request),
    ("default_branch", Request),
    ("forwarded", Request),
    ("function_seam", Seam),
    ("gate_response", Authz),
    ("http_cache", Service),
    ("member_authz", Authz),
    ("org_host_dispatch", Request),
    ("origin", Request),
    ("pagination", Service),
    ("preagg_cache_ctx", Service),
    ("preagg_context", Service),
    ("process_role", Request),
    ("rate_limit", Service),
    ("semantic_limit", Service),
    ("semantic_pin_seam", Seam),
    ("serve_mode", Request),
    ("shutdown", Request),
    ("sql_kind", Service),
    ("workspace_cache", Request),
    ("workspace_org", Request),
    ("workspace_repo_facts", Request),
];

/// Modules that have an owner, and the crate they belong in. Sorted.
/// Shrink-only: never add a row.
const BACKLOG: &[(&str, &str)] = &[
    ("app_lifecycle", "oxy-custom-apps"),
    ("audience", "oxy-custom-apps"),
    ("custom_app_env_request", "oxy-custom-apps"),
    ("custom_app_environment", "oxy-custom-apps"),
    ("custom_apps_host_dispatch", "oxy-custom-apps"),
    ("custom_domain_host", "oxy-custom-apps"),
    (
        "customer_runs",
        "usage reporting (admin usage_report's domain crate)",
    ),
    ("emails", "its own P2 vendor-client crate (SES)"),
    ("function_answer", "oxy-custom-apps"),
    ("internal_orgs", "oxy-tenancy"),
    (
        "llm_usage",
        "usage reporting (admin usage_report's domain crate)",
    ),
    ("notifications", "a notifications domain crate"),
    (
        "operating_graph",
        "oxy-tenancy, or a frontline domain crate",
    ),
    ("staging_pin_scope", "oxy-custom-apps"),
    ("ttl_lru", "oxy-custom-apps"),
    (
        "usage_period",
        "usage reporting (admin usage_report's domain crate)",
    ),
    ("usage_probe", "oxy-custom-apps"),
    ("widgets", "a home-widgets domain crate"),
];

/// The `pub mod` names `lib.rs` declares (a `#[cfg]` above one doesn't hide it).
fn declared_modules() -> BTreeSet<String> {
    include_str!("lib.rs")
        .lines()
        .filter_map(|line| line.trim().strip_prefix("pub mod "))
        .map(|rest| rest.trim_end_matches(';').trim().to_string())
        .collect()
}

#[test]
fn every_app_core_module_is_chartered_or_in_the_backlog() {
    let declared = declared_modules();
    let charter: BTreeSet<&str> = CHARTER.iter().map(|(m, _)| *m).collect();
    let backlog: BTreeSet<&str> = BACKLOG.iter().map(|(m, _)| *m).collect();

    let mut problems = Vec::new();
    for m in &declared {
        match (charter.contains(m.as_str()), backlog.contains(m.as_str())) {
            (false, false) => problems.push(format!(
                "new module `{m}`: if it passes the admission test in crates/app-core/CLAUDE.md, add it to CHARTER with its kind; if a context owns it, put it in that context's crate instead (BACKLOG never grows)"
            )),
            (true, true) => problems.push(format!("`{m}` is in both CHARTER and BACKLOG")),
            _ => {}
        }
    }
    for m in charter.iter().chain(backlog.iter()) {
        if !declared.contains(*m) {
            problems.push(format!(
                "`{m}` is listed but lib.rs no longer declares it: delete the entry"
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

#[test]
fn charter_lists_are_sorted_and_unique() {
    let charter: Vec<&str> = CHARTER.iter().map(|(m, _)| *m).collect();
    let backlog: Vec<&str> = BACKLOG.iter().map(|(m, _)| *m).collect();
    for list in [charter, backlog] {
        let mut sorted = list.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, list, "keep each list sorted and free of duplicates");
    }
}

#[test]
fn app_core_declares_no_entities_or_migrators() {
    // Admission test 4. Routes are not checked mechanically (tests here build
    // routers to exercise middleware); entities and migrators have no such excuse.
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).expect("read src dir").flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                && path.file_name() != Some("charter_tests.rs".as_ref())
            {
                let text = std::fs::read_to_string(&path).expect("read source");
                if text.contains("DeriveEntityModel") || text.contains("MigratorTrait") {
                    offenders.push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "entities belong in `entity`, migrators in `migration`: {offenders:?}"
    );
}
