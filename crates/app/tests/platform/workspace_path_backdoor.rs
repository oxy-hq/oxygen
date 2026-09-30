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
        "src/server/worktree_registry.rs",
        "the worktree registry itself: `get_state_dir().join(\"workspaces\")` is \
         where worktrees LIVE, so it cannot be handed a path by a manager built \
         from one. A fourth door the guard did not detect until its matcher was \
         tightened — the handler `get_worktree_status` takes only `Path<Uuid>`.",
    ),
    (
        "src/server/api/middlewares/workspace_context.rs",
        "the middleware that builds the manager — the path has to come from \
         somewhere before a manager exists",
    ),
    (
        "src/server/api/webhooks/toast.rs",
        "public router: bypasses workspace_context, so it resolves its own path. \
         Reads config from the compile boundary (#2816) and builds a slot-less manager",
    ),
    (
        "src/server/api/workspaces/ops.rs",
        "workspace registration: computes the path before the workspace exists",
    ),
    (
        "src/server/api/custom_apps_gates.rs",
        "custom-app execution — classified IdeOnly, runs the working copy",
    ),
    (
        "src/integrations/slack/workspace.rs",
        "the one place a Slack message resolves a workspace: boundary first, the \
         path only for the ide fallback. Resolution and execution both go through \
         it so the agent that is chosen and the run that executes it agree on a \
         revision",
    ),
    (
        "src/agentic_wiring/compile_dispatcher.rs",
        "the compile worker: compiling reads a working copy by definition, and the \
         worker refuses a Compile task on a node that owns none. A staging compile \
         resolves the BRANCH worktree (`effective_workspace_path`) before any \
         manager exists — the manager is what the compile produces",
    ),
    (
        "src/server/api/compile_staging.rs",
        "the staging-compile enqueue route, classified IdeOnly: it creates and \
         inspects the branch worktree (head SHA, clean check) to decide what to \
         compile, before any manager for that branch exists",
    ),
    (
        "src/integrations/slack/chart_render.rs",
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
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let allowed: Vec<&str> = ALLOWED.iter().map(|(path, _)| *path).collect();

    let mut offenders = Vec::new();
    let mut walk = vec![root.clone()];
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
