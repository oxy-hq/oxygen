//! `workspace_path()` is where a controlled manager becomes a raw `&Path`.
//!
//! Everything else in this design states its requirement in a type.
//! `ConfigManager<ReadOnly>` has no disk methods; `try_resolve_file` fails
//! with a reason; `require_root` refuses an absent root. All of it stops at the
//! moment a handler calls `workspace_path()`: it hands back a plain path, and
//! from there the handler can `std::fs` anything it likes with nothing watching.
//!
//! That is the fragmentation this branch set out to remove, and it is not
//! removed — it is *bounded*. The list below is the whole of it. It exists so
//! that:
//!
//! * a NEW handler reaching for a raw path fails CI instead of relying on a
//!   reviewer noticing, and
//! * the count is a number that can only go down.
//!
//! Sibling guard: `workspace_path_backdoor.rs` covers the *other* way in — the
//! free resolvers that produce a path before any manager exists. Between them,
//! every route to a workspace path is either typed or listed.
//!
//! **Adding an entry is a decision, not a formality.** Before you do, check
//! whether the read has a compiled artifact (`compiled_reader`), whether the
//! write belongs on the ide (`role_manifest`), and whether the path is a
//! *workspace* file at all — runtime artifacts (charts, results, caches) go
//! through `runtime_state_dir()` and S3, and are not this list's business.

use std::collections::BTreeMap;

/// Files permitted to take a raw workspace path out of a manager, and why.
///
/// The count is the point. Each entry is a place the type system stops being
/// able to help, so each needs a reason a reviewer can disagree with.
const ALLOWED: &[(&str, &str)] = &[
    (
        "src/server/api/middlewares/workspace_context.rs",
        "`WorkspaceRootWorkingCopy::root_path`. This is the extractor whose whole \
         purpose is to hand a handler the workspace ROOT with the requirement \
         stated in its signature — the one place the escape hatch is the point \
         rather than a leak. Every caller it serves came OFF the backdoor list.",
    ),
    (
        "src/server/api/workspaces/handlers.rs",
        "git surface: status, branches, commits, pull, reset. These ARE the \
         working copy — there is nothing to compile and no artifact to read \
         instead. Route-classified IdeOnly.",
    ),
    (
        "src/server/api/file.rs",
        "the IDE file editor: read, write, rename, delete inside the working \
         copy. Same — the working copy is the subject, not an implementation \
         detail. Fully-FS builder, classified IdeOnly.",
    ),
    (
        "src/server/api/data_repo.rs",
        "data-repo git operations. Fully-FS builder, IdeOnly.",
    ),
    (
        "src/server/api/modeling.rs",
        "dbt projects live in `modeling/` and have no compiled artifact. \
         IdeOnly; the LIST no longer degrades to `[]` when the ide is down.",
    ),
    (
        "src/server/api/app.rs",
        "BACKLOG. The compiled path is capability-free; these remain in \
         publish/unpublish, which write the working copy (IdeOnly).",
    ),
    (
        "src/server/api/metric_anomalies/scan.rs",
        "BACKLOG. `.monitor.yml` IS compiled and read at the pinned revision; \
         this is the FS fallback, now gated by `WorkspaceAbsent`. The handler \
         takes `WorkspaceManagerWorkingCopy`, so the route is IdeOnly and the \
         compiler holds it there.",
    ),
    (
        "src/server/api/database.rs",
        "the `.databases/` sync cache, written by the IdeOnly sync route. No \
         compiled substitute exists — `oxy-compile` carries `config.databases`, \
         not the synced schema. Degrades to `datasets: null`.",
    ),
    (
        "src/server/api/secrets.rs",
        "BACKLOG. Secret file resolution against the working copy.",
    ),
    (
        "src/server/api/integration.rs",
        "Looker synced metadata under the state dir. IdeOnly as of this branch.",
    ),
    (
        "src/server/api/data.rs",
        "BACKLOG. Path resolution for `sql_file` tasks.",
    ),
];

fn server_files() -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                let rel = path
                    .strip_prefix(env!("CARGO_MANIFEST_DIR"))
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if let Ok(body) = std::fs::read_to_string(&path) {
                    out.push((rel, body));
                }
            }
        }
    }
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut out = Vec::new();
    walk(&manifest.join("src/server"), &mut out);
    // Extracted from `src/server/api/documents`; still a request path of this
    // server, so still in scope. Labelled `../api-documents/src/…`.
    walk(&manifest.join("../api-documents/src"), &mut out);
    out
}

/// Every use of the escape hatch is on the list.
///
/// The failure message is the interesting part: it asks the three questions
/// that usually make the entry unnecessary, because the cheapest outcome here
/// is that the list does not grow.
#[test]
fn nothing_new_takes_a_raw_workspace_path() {
    let mut unlisted: Vec<String> = Vec::new();
    for (rel, body) in server_files() {
        if !body.contains(".workspace_path()") {
            continue;
        }
        if ALLOWED.iter().any(|(f, _)| *f == rel) {
            continue;
        }
        unlisted.push(rel);
    }
    unlisted.sort();
    assert!(
        unlisted.is_empty(),
        "these files call `workspace_path()` and are not on the list:\n  {}\n\n\
         `workspace_path()` turns a manager that states its requirements into a \
         bare `&Path`, after which nothing checks anything. Before adding an \
         entry, three questions:\n\
         \x20 1. Is there a compiled artifact? `compiled_reader` may already \
         serve this — read it at `config_manager.revision_id()`.\n\
         \x20 2. Is this a workspace FILE, or a runtime artifact? Charts, \
         results and caches belong to `runtime_state_dir()` + S3, not here.\n\
         \x20 3. If it genuinely needs the working copy, is the route IdeOnly?\n\n\
         If the answer to all three leaves you needing the path, add the file to \
         ALLOWED with the reason.",
        unlisted.join("\n  ")
    );
}

/// The list shrinks when a file stops needing the path.
///
/// Without this the allowlist only ever grows, and a stale entry silently
/// re-permits a file that had been cleaned up.
#[test]
fn the_list_has_no_stale_entries() {
    let files: BTreeMap<String, String> = server_files().into_iter().collect();
    let mut stale: Vec<&str> = Vec::new();
    for (rel, _) in ALLOWED {
        match files.get(*rel) {
            Some(body) if body.contains(".workspace_path()") => {}
            _ => stale.push(rel),
        }
    }
    assert!(
        stale.is_empty(),
        "these are on the list but no longer call `workspace_path()` — remove \
         them, so the count keeps meaning something:\n  {}",
        stale.join("\n  ")
    );
}

/// A ratchet on the count itself.
///
/// The per-file list catches a new FILE. It does not catch an existing file
/// growing three more call sites, which is how fragmentation actually returns.
/// Lower this number as the backlog clears; never raise it.
#[test]
fn the_escape_hatch_does_not_widen() {
    // 43 -> 44: `delete_workspace` now reaches its path through a manager
    // instead of the free resolver. That is a rise here and a fall on
    // `workspace_path_backdoor` (8 -> 7), and it is the trade worth making — an
    // escape-hatch use is visible to the type system, a backdoor use is not.
    // Raising this number needs a reason; this is it.
    // 45 -> 39: every artifact read moved the boundary/disk choice into
    // `ConfigManager`, so its callers no longer resolve a raw path. The one site
    // that still needed an absolute path (`automation_run`'s filesystem
    // fallback) goes through `resolve_file`, which checks containment.
    const CEILING: usize = 36;
    let total: usize = server_files()
        .iter()
        .map(|(_, body)| body.matches(".workspace_path()").count())
        .sum();
    assert!(
        total <= CEILING,
        "`workspace_path()` is used {total} times in src/server, over the \
         ceiling of {CEILING}. This is the measure of how much workspace access \
         still bypasses the type system. Lower the ceiling when you remove a \
         use; raising it needs a reason in the PR.",
    );
}
