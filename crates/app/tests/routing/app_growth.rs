//! `oxy-app` takes no new context (rule S8 in `internal-docs/domain-boundaries.md`).
//!
//! `oxy-server` is the composition root and `oxy-app` the platform runtime, plus the
//! bounded contexts it already holds until each moves into its own `api-<context>`
//! surface over a domain crate. Those contexts arrived one module at a time, so this
//! test closes the door at the place they arrive: the top-level modules of
//! `crates/app/src/`, `src/server/` and `src/server/api/`.
//!
//! The lists below are what existed on 2026-11-06. They are a backlog, not an
//! exemption list (the stance of `surface_contract.rs` and `authz_boundaries.rs`):
//! - **A new module fails.** Start the feature in its own surface crate instead
//!   (`domain-boundaries.md`, "HTTP surface crates", S1–S8). Splitting a module
//!   that is already listed into a sub-module dir (Rung 1) is fine; that happens
//!   below this level.
//! - **A listed module that no longer exists fails**, so the list shrinks as
//!   contexts leave and nothing can quietly reuse a freed name. Delete the entry.
//!
//! Why the door matters for the build, not only the boundary: about half of every
//! `oxy-app` recompile is whole-crate work (metadata encoding, the rlib write, macro
//! expansion, incremental persistence) that grows with the crate, whichever file was
//! edited ("Build reality" in the same doc).

use std::collections::BTreeSet;
use std::path::Path;

const SRC: &[&str] = &[
    "agentic_wiring",
    "airway_boot",
    "airway_boot_tests",
    "cli",
    "integrations",
    "managed_connection_resolver",
    "observability_boot",
    "observability_setup",
    "server",
    "surface",
];

const SERVER: &[&str] = &[
    "admission",
    "anomaly_notify",
    "api",
    "audit_anchor",
    "auth_provider_check",
    "authz",
    "build_identity",
    "builder_test_runner",
    "compile_config_gate",
    "compile_git",
    "compile_maintenance",
    "compile_oltp",
    "compile_reconcile",
    "compile_request",
    "compile_trigger",
    "compile_worker",
    "compile_worker_tests",
    "factory_access",
    "factory_replay",
    "feature_flags",
    "git_fetch_maintenance",
    "health_eval_executor",
    "http_cache",
    "ide_proxy",
    "metrics_server",
    "preagg_executor",
    "preagg_freshness",
    "preagg_generation",
    "preagg_ledger",
    "preagg_promote",
    "preagg_rebuild",
    "preagg_retract",
    "preagg_workspace",
    "previews",
    "queued_analytics",
    "role_manifest",
    "role_manifest_tests",
    "role_middleware",
    "role_middleware_tests",
    "route_catalog",
    "router",
    "runtime_artifact",
    "servability",
    "service",
    "shared_state_check",
    "task_router_listener",
    "test_support",
    "test_support_tests",
    "token_sweep",
    "token_usage_flush",
    "worker_health",
    "worker_metrics",
    "worker_runtime",
    "workspace_fs",
    "worktree_registry",
];

const SERVER_API: &[&str] = &[
    "admin",
    "agent",
    "api_keys",
    "apps",
    "artifacts",
    "auth",
    "automation",
    "billing",
    "catalog",
    "chart",
    "chat",
    "cohort",
    "competitors",
    "compile",
    "compile_staging",
    "compile_status_tests",
    "custom_apps_gates",
    "data",
    "database",
    "execution_analytics",
    "file",
    "foot_traffic",
    "foot_traffic_seed",
    "healthcheck",
    "integration",
    "local_setup",
    "metric_anomalies",
    "metric_tree",
    "metric_tree_baseline",
    "metric_tree_groups",
    "metric_tree_inputs",
    "metric_tree_probe",
    "metric_tree_projection",
    "metrics",
    "middlewares",
    "notifications",
    "oidc_exchange",
    "operating_graph",
    "org_api_access",
    "org_subdomain",
    "org_teams",
    "partner_publish_consent",
    "pipeline",
    "preagg",
    "projects",
    "public_rate_limit",
    "result_files",
    "schedules",
    "schedules_tests",
    "secrets",
    "semantic",
    "semantic_scan",
    "thread",
    "traces",
    "typed_stream",
    "user",
    "user_tokens",
    "video",
    "webhooks",
    "work",
    "workspace_logo",
    "workspace_members",
    "workspace_members_tests",
    "workspace_org",
    "workspace_oxy_access",
    "workspace_previews",
    "workspaces",
    "world_model",
    "world_model_graph",
];

/// The module names a directory declares at its top level: `foo.rs` and `foo/`
/// (with any `.rs` under it) are both `foo`. `mod.rs` / `lib.rs` are the parent.
fn top_level_modules(dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if path.is_dir() {
            if has_rust(&path) {
                out.insert(name);
            }
        } else if let Some(stem) = name.strip_suffix(".rs")
            && !matches!(stem, "mod" | "lib" | "main")
        {
            out.insert(stem.to_string());
        }
    }
    out
}

fn has_rust(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let p = e.path();
            if p.is_dir() {
                has_rust(&p)
            } else {
                p.extension().is_some_and(|x| x == "rs")
            }
        })
}

/// `(new, stale)` for one directory against its closed list.
fn diff(dir: &Path, allowed: &[&str]) -> (Vec<String>, Vec<String>) {
    let found = top_level_modules(dir);
    let allowed: BTreeSet<String> = allowed.iter().map(|s| s.to_string()).collect();
    (
        found.difference(&allowed).cloned().collect(),
        allowed.difference(&found).cloned().collect(),
    )
}

#[test]
fn oxy_app_takes_no_new_top_level_module() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut problems = Vec::new();
    for (rel, allowed) in [("", SRC), ("server", SERVER), ("server/api", SERVER_API)] {
        let dir = src.join(rel);
        let (new, stale) = diff(&dir, allowed);
        let at = if rel.is_empty() {
            "src".to_string()
        } else {
            format!("src/{rel}")
        };
        for n in new {
            problems.push(format!(
                "new module `{at}/{n}`: oxy-app takes no new context (S8). Put it in its own \
                 `crates/api-<context>` surface over a domain crate; see \
                 internal-docs/domain-boundaries.md, \"HTTP surface crates\"."
            ));
        }
        for s in stale {
            problems.push(format!(
                "`{at}/{s}` is listed but gone: delete it from the list in \
                 tests/routing/app_growth.rs so the name cannot come back."
            ));
        }
    }
    assert!(problems.is_empty(), "\n{}\n", problems.join("\n"));
}

#[test]
fn lists_are_sorted_and_unique() {
    for list in [SRC, SERVER, SERVER_API] {
        let mut sorted = list.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, list, "keep each list sorted and free of duplicates");
    }
}
