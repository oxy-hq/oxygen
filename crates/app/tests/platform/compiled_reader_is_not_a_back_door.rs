//! `ConfigManager` decides which source an artifact read uses. This keeps that
//! true.
//!
//! `compiled_reader` used to be the way every handler asked "is there a compiled
//! row for this?" and then decided what to do about the answer — nine call sites
//! spelling the same three-arm match, and two of them getting it wrong in a way
//! that reported a platform fault as the customer's configuration. The reads
//! moved onto `ConfigManager`, which matches on `Origin` once and returns a
//! typed error instead of an empty list.
//!
//! Nothing stops the next handler reaching past it. This does.
//!
//! Same shape as `workspace_path_escape_hatch.rs`: a source scan with an
//! allowlist where each entry carries its reason, and where a STALE entry fails
//! too — otherwise the list only grows and stops meaning anything.

use std::path::{Path, PathBuf};

/// Files that may name `compiled_reader`, and why.
///
/// Two kinds only. Anything else belongs on `ConfigManager`.
///
///   1. **Resolving the request's `Origin`** — runs before a manager exists, and
///      needs the process role plus a `git` call for the default branch, neither
///      of which belongs in `crates/core`.
///   2. **No manager to have** — the compile worker checking a revision it just
///      wrote, and two decisions taken before there is anything to read from.
const ALLOWED: &[(&str, &str)] = &[
    (
        "src/server/api/middlewares/workspace_context.rs",
        "Resolves the revision the whole request is pinned to, once, and stamps \
         it into `Origin`. This is the source of the answer every other read \
         uses.",
    ),
    (
        "src/server/router/recovery.rs",
        "Builds a manager outside the HTTP path, so it resolves the revision the \
         same way the middleware does.",
    ),
    (
        "src/server/api/custom_apps_gates.rs",
        "Same: builds the per-request project context for the public custom-app \
         router, which has no workspace middleware.",
    ),
    (
        "src/server/api/webhooks/toast.rs",
        "Same, on the public webhook router.",
    ),
    (
        "src/integrations/slack/workspace.rs",
        "Same, off the HTTP path entirely.",
    ),
    (
        "src/server/api/semantic.rs",
        "Doc comment only — explains where branch semantics come from. If this \
         ever becomes a call, it is a bug: the handler has a manager.",
    ),
    (
        "src/server/compile_worker.rs",
        "Checks the revision it JUST wrote, before anything reads it. A manager \
         reads the promoted revision, which is a different question.",
    ),
    (
        "src/server/serve_safety.rs",
        "Decides whether the fleet can serve a workspace locally or must proxy to \
         the ide. A routing decision, taken before there is a manager.",
    ),
    (
        "src/server/api/admin/workspace_health/smoke/config.rs",
        "Reads the compiled config BEFORE building a workspace context, \
         deliberately — a promoted revision with no `smoke_test` block is \
         authoritative, and building a context to re-learn that on every eval \
         pass is the cost this avoids.",
    ),
    (
        "src/server/api/custom_apps_functions/preflight.rs",
        "Runs inside `oxy migrate`, before any server or manager exists, and takes \
         the `databases` key from the compiled config of every workspace that has \
         a live custom app — the promoted revision the serve fleet reads. Building \
         a workspace context per workspace in a deploy hook to learn a list of \
         names is the cost this avoids.",
    ),
];

fn app_src() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` under `src`, as (repo-relative path, contents).
fn source_files() -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
                && let Ok(body) = std::fs::read_to_string(&path)
            {
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                out.push((format!("src/{rel}"), body));
            }
        }
    }
    let root = app_src();
    let mut out = Vec::new();
    walk(&root, &root, &mut out);
    out
}

fn mentions_compiled_reader(body: &str) -> bool {
    body.contains("compiled_reader::")
}

#[test]
fn nothing_new_reaches_past_config_manager() {
    let offenders: Vec<String> = source_files()
        .into_iter()
        .filter(|(rel, body)| {
            rel != "src/server/api/compiled_reader.rs"
                && mentions_compiled_reader(body)
                && !ALLOWED.iter().any(|(f, _)| f == rel)
        })
        .map(|(rel, _)| rel)
        .collect();

    assert!(
        offenders.is_empty(),
        "these files name `compiled_reader` and are not on the list:\n  {}\n\n\
         Reading an artifact through `compiled_reader` means deciding \
         compiled-vs-disk at the call site, which is what `ConfigManager` is \
         for. Before adding an entry:\n\
         \x20 1. Does `ConfigManager` already answer this? `list_apps`, \
         `resolve_app`, `list_automations`, `automation_definition`, \
         `monitor_config`, `semantics_scan` and friends cover every artifact \
         kind, and each owns BOTH arms — there is no `compiled_*` left to \
         call.\n\
         \x20 2. Do you have a manager? If the caller has a `WorkspaceManager`, \
         use it — the middleware already pinned its revision.\n\
         \x20 3. If you genuinely have no manager (you are resolving the \
         `Origin`, or checking a revision you just wrote), add the file WITH the \
         reason.",
        offenders.join("\n  ")
    );
}

/// Without this the allowlist only ever grows. A stale entry is worse than a
/// missing one: it reads as "this file needs the escape hatch" long after it
/// stopped needing it, and the next person copies it.
#[test]
fn the_list_has_no_stale_entries() {
    let files = source_files();
    let stale: Vec<&str> = ALLOWED
        .iter()
        .map(|(f, _)| *f)
        .filter(|f| {
            !files
                .iter()
                .any(|(rel, body)| rel == f && mentions_compiled_reader(body))
        })
        .collect();

    assert!(
        stale.is_empty(),
        "these are on the list but no longer name `compiled_reader` — remove \
         them:\n  {}",
        stale.join("\n  ")
    );
}

/// The counter-guard: if someone "fixes" the test above by deleting the module,
/// this fails. `compiled_reader` still owns the one question `ConfigManager`
/// cannot answer — which revision this request reads — and that has to keep
/// living somewhere the process role and `git` are reachable.
#[test]
fn the_origin_resolver_still_exists() {
    let path = app_src().join("server/api/compiled_reader.rs");
    let body =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    for expected in [
        "pub async fn resolve_request_revision",
        "pub async fn with_pinned_revision",
    ] {
        assert!(
            body.contains(expected),
            "`{expected}` should still live in `compiled_reader` — resolving the \
             request's revision needs the process role and a `git` call for the \
             default branch, so it cannot move into `crates/core` with the \
             queries."
        );
    }
}
