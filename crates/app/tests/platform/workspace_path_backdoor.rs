//! The one way to reach the working copy without asking the type system.
//!
//! `ConfigManager<ReadOnly>` cannot hand out a workspace path. But
//! `oxy::adapters::workspace::{resolve_workspace_path, effective_workspace_path}`
//! are free functions that return a `PathBuf` from a database column, with no
//! capability involved and — importantly — **without stat-ing it**. A handler
//! that calls one gets a path that always resolves and, on a stateless replica,
//! points at a directory that is not there.
//!
//! That is exactly how the Slack handlers reached the disk: `resolve_workspace_path`,
//! then `WorkspaceBuilder`, then a filesystem walk that returned nothing. The
//! capability parameter never saw it, because the path never came from a manager.
//!
//! A `clippy.toml` ban is the wrong tool here — `disallowed-methods` is
//! workspace-wide and cannot be scoped to the handler layer, and
//! `crates/app/src/server` has ~100 legitimate `fs::` calls against the state
//! dir and temp files. This list is small enough to be read instead.

use std::path::PathBuf;

/// Call sites permitted to resolve a workspace path directly, and why.
///
/// A backlog, not an exemption list: each of these is a place where the path is
/// obtained before any manager exists, so the capability cannot be consulted yet.
/// Anything else should take the path from a `ConfigManager<WorkingCopy>`, which states
/// the requirement in its type.
const ALLOWED: &[(&str, &str)] = &[
    (
        // `oxy-workspace-runtime`'s since 2026-11-22 (W7), was
        // `src/server/api/middlewares/workspace_context.rs`.
        "../workspace-runtime/src/request/workspace_context.rs",
        "the middleware that builds the manager — the path has to come from \
         somewhere before a manager exists",
    ),
    (
        "../api-world-model/src/toast_webhook.rs",
        "public router: bypasses workspace_context, so it resolves its own path. \
         Reads config from the compile boundary (#2816) and builds a slot-less manager \
         (`oxy-api-world-model`'s since 2026-11-15, was `src/server/api/webhooks/toast.rs`)",
    ),
    (
        "../api-workspaces/src/workspaces/ops.rs",
        "workspace registration: computes the path before the workspace exists \
         (`oxy-api-workspaces`' since 2026-11-16, was `src/server/api/workspaces/ops.rs`)",
    ),
    (
        // `oxy-workspace-runtime`'s since 2026-11-22 (W6a), was
        // `src/server/api/custom_apps_gates.rs`.
        "../workspace-runtime/src/custom_apps_gates.rs",
        "the custom-app data plane and function context, which run outside \
         workspace_context: boundary first, the path only for the fallback on a \
         pod that holds the working copy. A pod that does not is handed the \
         `workspaces.path` column as a label and never calls the resolver",
    ),
    (
        "../api-slack/src/workspace.rs",
        "the one place a Slack message resolves a workspace: boundary first, the \
         path only for the ide fallback. Resolution and execution both go through \
         it so the agent that is chosen and the run that executes it agree on a \
         revision",
    ),
    (
        // `oxy-workspace-runtime`'s since 2026-11-22 (W4), was
        // `src/agentic_wiring/compile_dispatcher.rs`.
        "../workspace-runtime/src/compile_dispatcher.rs",
        "the compile worker: compiling reads a working copy by definition, and the \
         worker refuses a Compile task on a node that owns none. A staging compile \
         resolves the BRANCH worktree (`effective_workspace_path`) before any \
         manager exists — the manager is what the compile produces",
    ),
    (
        // `oxy-compile-runtime`'s `request` since 2026-11-22 (was
        // `src/server/compile_request/working_copy.rs`).
        "../compile-runtime/src/request/working_copy.rs",
        "the branch compile's working-copy arm, for a branch GitHub does not have: \
         it creates and inspects the branch worktree (head SHA, clean check) to \
         decide what to compile, before any manager for that branch exists. Only \
         reached on a process that owns working copies; a process that owns none \
         refuses before calling it",
    ),
    (
        "../api-slack/src/chart_render.rs",
        "renders a chart into the local state dir",
    ),
];

/// Does this file reach a workspace path without a manager?
///
/// Bare substring matching was wrong in both directions, which is worth stating
/// because this guard shipped that way. It listed `cli/commands/seed.rs`, whose
/// `resolve_workspace_path` is its own private function canonicalising a CLI
/// argument, and `agentic_wiring/compile_dispatcher.rs`, which calls
/// `oxy_compile::resolve_workspace_path` — a different function in a different
/// crate that takes a `&db`. Neither is the backdoor. Both would have sat on the
/// allowlist forever, since the counter-guard used the same loose match and so
/// never called them stale.
///
/// It also missed one: `oxy::state_dir::get_state_dir()` reaches node-local
/// state with no manager and no workspace row at all.
fn resolves_a_workspace_path(src: &str) -> bool {
    // The DB-backed resolvers, qualified or imported from `oxy::adapters::workspace`.
    let imports_adapter = src.contains("oxy::adapters::workspace");
    let db_resolver = ["resolve_workspace_path(", "effective_workspace_path("]
        .iter()
        .any(|call| {
            src.contains(call) && (imports_adapter || src.contains(&format!("workspace::{call}")))
        })
        && !src.contains("pub async fn effective_workspace_path(");

    // The state dir: a different door to the same class of node-local path.
    let state_dir = src.contains("state_dir::get_state_dir()");

    db_resolver || state_dir
}

#[test]
fn nothing_new_resolves_a_workspace_path_by_hand() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.join("src");
    let allowed: Vec<&str> = ALLOWED.iter().map(|(path, _)| *path).collect();

    let mut offenders = Vec::new();
    // `oxy-api-documents` was extracted from `src/server/api/documents` and
    // still serves requests from this binary, so the guard follows it. Paths
    // there read `../api-documents/src/…`. `oxy-api-airway`, `oxy-api-slack`
    // `oxy-api-operating-graph` and `oxy-api-world-model` likewise, and
    // `oxy-world-model`, where the
    // World Model Graph's cores went down to.
    let mut walk = vec![
        root.clone(),
        manifest.join("../api-documents/src"),
        manifest.join("../api-airway/src"),
        manifest.join("../api-slack/src"),
        manifest.join("../api-operating-graph/src"),
        manifest.join("../api-world-model/src"),
        manifest.join("../world-model/src"),
        // The previews' upper half, `oxy-api-previews` since 2026-11-14.
        manifest.join("../api-previews/src"),
        // The automation listing and the schedules, `oxy-api-automations`
        // since 2026-11-15.
        manifest.join("../api-automations/src"),
        // The workspace's databases, secrets and OAuth connects (the last
        // out of `src/integrations`, 2026-11-21), `oxy-api-workspace-config`
        // since 2026-11-15.
        manifest.join("../api-workspace-config/src"),
        // The health probes and route discovery, `oxy-server`'s `core_routes`
        // since 2026-11-16.
        manifest.join("../server/src/core_routes"),
        // The run drivers and the periodic tick, `oxy-server`'s `drivers`
        // since 2026-11-23 (decompose BL1, out of `src/server/router`).
        manifest.join("../server/src/drivers"),
        // The router assembly, its middlewares and the local `/setup` handlers,
        // and the admin console shell, `oxy-server`'s since 2026-11-23
        // (decompose LIFT, out of `src/server/{router,api}`, walked above).
        manifest.join("../server/src/router"),
        manifest.join("../server/src/admin"),
        // The demo seed (`oxy dev seed`), `oxy-server`'s `dev/seed` since
        // 2026-11-24 (decompose SD, out of `src/cli/commands/seed*`, walked above).
        manifest.join("../server/src/dev"),
        // The conversation surface, `oxy-api-threads` since 2026-11-16.
        manifest.join("../api-threads/src"),
        // The workspaces surface, `oxy-api-workspaces` since 2026-11-16.
        manifest.join("../api-workspaces/src"),
        // The semantic surface, `oxy-api-semantic` since 2026-11-16.
        manifest.join("../api-semantic/src"),
        // The usage report's domain half, `oxy-usage-report` since 2026-11-20
        // (out of `src/server/api/admin/usage_report`).
        manifest.join("../usage-report/src"),
        // Its surface (the routes, recipients, delivery and the weekly job),
        // `oxy-api-billing` since admin split PR 8b (out of the same directory).
        manifest.join("../api-billing/src"),
        // The pre-aggregation runtime's domain half, `oxy-preagg` since
        // 2026-11-20 (out of `src/server/preagg_{freshness,generation,
        // rebuild,retract,promote}`).
        manifest.join("../preagg/src"),
        // The workspace runtime, `oxy-workspace-runtime` since 2026-11-21
        // (its leaves, out of `src/agentic_wiring` and `src/server`).
        manifest.join("../workspace-runtime/src"),
        // The compile boundary's runtime leaves, `oxy-compile-runtime` since
        // 2026-11-21 (out of `src/server/compile_{git,oltp,config_gate,
        // maintenance}`), and the branch compile since 2026-11-22 (out of
        // `src/server/compile_request`).
        manifest.join("../compile-runtime/src"),
        // Workspace previews' rows, `oxy-previews` since 2026-11-22 (out of
        // `src/server/previews` and the custom-app preview settle).
        manifest.join("../previews/src"),
        // The feature-flag runtime's domain half, `oxy-feature-flags` since
        // 2026-11-20 (out of `src/server/feature_flags`).
        manifest.join("../feature-flags/src"),
        // Insights delivery, out of `src/server/anomaly_notify` on 2026-11-21:
        // the enqueue into `oxy-metric-monitoring`, the executor and its
        // destinations into `oxy-api-metric-monitoring`.
        manifest.join("../metric-monitoring/src"),
        manifest.join("../api-metric-monitoring/src"),
        // `oxy test`, its event handler, the reporters and the engine,
        // `oxy-api-eval` since 2026-11-20 (out of `src/cli`, `src/server/service`,
        // `src/integrations`).
        manifest.join("../api-eval/src"),
        // The feature-flag routes, `oxy-api-admin`'s since 2026-11-21 (out of
        // `src/server/feature_flags/routes.rs`), with the rest of the staff
        // console crate: Postgres and the task queue only.
        manifest.join("../api-admin/src"),
    ];
    while let Some(dir) = walk.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            if !resolves_a_workspace_path(&src) {
                continue;
            }
            let rel = path
                .strip_prefix(PathBuf::from(env!("CARGO_MANIFEST_DIR")))
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if !allowed.contains(&rel.as_str()) {
                offenders.push(rel);
            }
        }
    }
    // Three files `oxy-api-tenancy` took from `src/server/api` on 2026-11-16
    // (the org subdomain's status, the logo, the consent switch), by name: the
    // rest of that crate provisions and clones working copies on purpose.
    for rel in [
        "../api-tenancy/src/org_subdomain.rs",
        "../api-tenancy/src/workspace_logo.rs",
        "../api-tenancy/src/partner_publish_consent.rs",
    ] {
        let src = std::fs::read_to_string(manifest.join(rel))
            .unwrap_or_else(|e| panic!("{rel} moved, and this guard lost it: {e}"));
        if resolves_a_workspace_path(&src) && !allowed.contains(&rel) {
            offenders.push(rel.to_string());
        }
    }

    assert!(
        offenders.is_empty(),
        "these resolve a workspace path without going through a \
         `ConfigManager<WorkingCopy>`:\n  {}\n\n\
         `resolve_workspace_path` reads `workspaces.path` from the database and \
         does NOT stat it, so on a stateless replica it returns a directory that \
         is not there and every read comes back empty. Take the path from a \
         `ConfigManager<WorkingCopy>` — which says in its type that this process owns a \
         disk — or add an entry to ALLOWED explaining why the path has to be \
         resolved before a manager exists.",
        offenders.join("\n  ")
    );
}

/// Counter-guard: an allowlist that names files which no longer call anything is
/// dead weight that makes the real list harder to read.
#[test]
fn the_allowlist_has_no_stale_entries() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let stale: Vec<&str> = ALLOWED
        .iter()
        .filter(|(rel, _)| {
            let Ok(src) = std::fs::read_to_string(base.join(rel)) else {
                return true; // file gone
            };
            !resolves_a_workspace_path(&src)
        })
        .map(|(rel, _)| *rel)
        .collect();

    assert!(
        stale.is_empty(),
        "these are allowlisted but no longer resolve a workspace path — remove \
         them:\n  {}",
        stale.join("\n  ")
    );
}
