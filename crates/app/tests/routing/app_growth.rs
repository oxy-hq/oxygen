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
    "cli",
    // 2026-11-11: re-exports only, no code. What `oxy-server`'s `serve` /
    // `start` / `worker` take from here, the binary's counterpart of
    // `surface`. It shrinks to nothing as the router and the base leave.
    "host",
    "managed_connection_resolver",
    "server",
    // `surface`, the prelude the surface crates took from oxy-app, went on
    // 2026-11-24 with its last entry, the demo seed (decompose SD).
];

const SERVER: &[&str] = &[
    "api",
    // `audit_anchor` and `token_usage_flush` moved up into `oxy-server`'s
    // `drivers` with the run drivers on 2026-11-23 (decompose BL1), and with
    // them `test_support`, the in-source DB harness shim they were the last
    // callers of.
    // `auth_provider_check` moved up into `oxy-server` with the router on
    // 2026-11-23 (decompose LIFT).
    "authz",
    "build_identity",
    // The compile boundary's runtime leaves (`compile_config_gate`,
    // `compile_git`, `compile_maintenance`, `compile_oltp`) left for
    // `oxy-compile-runtime` on 2026-11-21, re-exported in `server/mod.rs`;
    // `compile_request` followed them on 2026-11-22 (as `request`), and
    // `compile_reconcile` the same day (as `reconcile`), and `compile_worker`
    // (with `compile_worker_tests`) the same day too (as `worker`).
    // The Factory-access gate left for `oxy-workspace-runtime::request` on
    // 2026-11-22 (decompose W7), re-exported in `server/mod.rs`.
    "http_cache",
    // Pod placement (`ide_proxy`, `servability`, and `role_manifest`'s and
    // `role_middleware`'s code) left for `oxy-app-core::pods` on 2026-11-22,
    // re-exported in `server/mod.rs`; the two files that stay hold the test
    // installer and the tests that build the real router. The task-router
    // listener (`task_router_listener`) left the same day, as `listener`.
    // The pre-aggregation runtime's domain half (`freshness`, `generation`,
    // `rebuild`, `retract`) left for `oxy-preagg` on 2026-11-20, and the
    // post-promote nudge (`preagg_promote`, now `oxy_preagg::promote`,
    // re-exported in `server/mod.rs`) followed it the same day, and so did
    // the cycle's body (`oxy_preagg::cycle`). `preagg_executor`, the executor
    // shell, left for `oxy-workspace-runtime` on 2026-11-22 (W4) with the
    // `OxyProjectContext` its `CycleWorkspace` impl is for (re-exported in
    // `server/mod.rs`).
    // `preagg_workspace` (the cycle's workspace manager) left for
    // `oxy-workspace-runtime` on 2026-11-22 (re-exported in `server/mod.rs`).
    "previews",
    // The queued analytics run (`queued_analytics`) left for
    // `oxy-workspace-runtime` on 2026-11-23 (W6b), re-exported in
    // `server/mod.rs`.
    // `role_manifest`, `role_manifest_tests`, `role_middleware`,
    // `role_middleware_tests` and `router` (the router assembly, with the test
    // installer and the tests that build it) moved up into `oxy-server` on
    // 2026-11-23 (decompose LIFT). The route table's types went down into
    // `oxy-route-catalog` the same day (decompose RC).
    // The runtime-artifact S3 mirror left for `oxy-workspace-runtime` on
    // 2026-11-21 (re-exported in `server/mod.rs` until R1). `service`, the
    // re-exports of `oxy::service` and `oxy::types`, was deleted in R1 when
    // its last caller imported `oxy::service` directly.
    // `worker_health`, `worker_metrics`, `worker_runtime` and
    // `shared_state_check` moved up into `oxy-server` on 2026-11-22 (UP2).
    // `workspace_fs`, a re-export of `oxy-workspace-fs` with no caller left,
    // was deleted on 2026-11-23 (decompose SHM).
];

const SERVER_API: &[&str] = &[
    // `admin` (the console shell) and `local_setup` moved up into
    // `oxy-server` with the router on 2026-11-23 (decompose LIFT).
    // Re-exports only since decompose B2 (the gate is `oxy_billing::gate`,
    // the past-due dispatcher `oxy_api_billing::billing::notify`).
    "billing",
    // The data plane's gate and the semantic boundary (out of `projects`) left
    // for `oxy-workspace-runtime` on 2026-11-22 (W6a), re-exported in
    // `api/mod.rs` and `projects/mod.rs`.
    // Re-exports only since decompose LIFT: the router-only middlewares went up
    // into `oxy-server`'s `router::middlewares`.
    "middlewares",
    "projects",
    // The typed-stream sinks left for `oxy-workspace-runtime` on 2026-11-21
    // (re-exported in `api/mod.rs`).
    // `workspace_org`, a re-export of `oxy-app-core::workspace_org` with no
    // caller left, was deleted on 2026-11-23 (decompose SHM).
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
