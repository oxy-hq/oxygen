//! `WorkspaceContext::workspace_path()` must be filtered by `is_dir()` before
//! anything opens what it returns.
//!
//! ## The trap
//!
//! `workspace_path()` is an `Option`, which reads as "`Some` = this node has
//! the files". It does not mean that. It reports whether the manager
//! **declares** a working copy, and `OxyProjectContext` holds a
//! `WorkspaceManager<WorkingCopy>` whose slot is always full —
//! `effective_workspace_path` hands back the database column without stat-ing
//! it. On a `serve` or `worker` replica with no `/workspace` volume it is
//! therefore `Some`, naming a directory that is not there.
//!
//! Every `ok_or_else(|| "this node holds no workspace files")` written against
//! the bare `Option` is consequently dead on exactly the node it was written
//! for, and the real failure arrives later and shapeless: a raw ENOENT, or —
//! worse — an empty scan reported as an empty workspace.
//!
//! ## Why a test and not a comment
//!
//! This has now been the same defect three times, in three crates, found by
//! grepping rather than by anything failing:
//!
//! - `pipeline_ref.rs` — got the filter when airway runs moved to the worker.
//! - `step_executor.rs::load_sql_body` (`sql_file`) — oxygen-internal#3151.
//!   Took a pipeline down on oxy-dev: `failed to read SQL file …: No such file
//!   or directory`, three OOM-free retries, then dead-lettered.
//! - `step_executor.rs::execute_semantic_query` — #3151. Worse than the above,
//!   because an absent semantic directory yields ZERO views rather than an
//!   error, so the step returned a wrong answer instead of failing.
//!
//! A fourth caller will be written. This test is what makes that a build
//! failure instead of an incident.
//!
//! ## What it does NOT check
//!
//! Only that the filter is present at the call site. It cannot tell you that
//! the path is then used correctly, and it deliberately does not look at
//! `oxy-app`'s `ConfigManager::workspace_path()` — a different method, with a
//! different signature (`&Path`, not `Option`), governed by route
//! classification rather than by this rule. See `oxy-route-classification`.
//!
//! ## Sibling guards — three doors, one room
//!
//! * `workspace_path_backdoor.rs` — the free resolvers that mint a path before
//!   any manager exists (`resolve_workspace_path`).
//! * `workspace_path_escape_hatch.rs` — `oxy-app` handlers turning a managed
//!   `ConfigManager` back into a raw `&Path`, scoped to `src/server`.
//! * **this file** — the agentic task-execution path, where the *port*'s
//!   `Option` is mistaken for a presence check.
//!
//! The first two bound the HANDLER layer, where route classification is the
//! backstop if they leak. This one bounds the WORKER layer, which has no such
//! backstop: a task executor runs wherever the queue sends it.

use std::fs;
use std::path::{Path, PathBuf};

/// The shape we object to: `workspace_path()` NOT followed by a containment
/// filter.
const NEEDLE: &str = ".workspace_path()";

/// How many lines after the needle to search for a sanctioned form.
///
/// Not 0. `rustfmt` splits a method chain the moment it exceeds the line width,
/// so the very call sites this test protects — which carry a `.filter(..)`, a
/// `.map(..)` and an `.ok_or_else(..)` — routinely land with `.workspace_path()`
/// alone on its line and the filter beneath it. A same-line-only match reported
/// every one of them as a violation, which would have taught the next reader
/// that the test cries wolf.
const LOOKAHEAD: usize = 3;

/// Forms that make a call site safe, searched within [`LOOKAHEAD`] lines.
const SANCTIONED: &[&str] = &[
    // The filter itself — the point of the test.
    ".filter(|p| p.is_dir())",
    // The declaration of the method, and doc references to it.
    "fn workspace_path(",
    // Stringified into a render context or a log field. A path being turned
    // into text is not a path being opened, and the template that later reads
    // it is covered wherever it actually opens something.
    "to_string_lossy()",
];

/// Call sites allowed to use the bare `Option`, and why.
///
/// **This list is a backlog, not an exemption list.** Before adding to it, ask
/// whether you are taking the shortcut this test exists to object to. A genuine
/// entry either does not touch the filesystem at all, or is on a path that
/// provably only runs where the files exist.
const ALLOWED: &[(&str, &str)] = &[
    // Arithmetic only. `make_workspace_relative` returns a relative path
    // unchanged, so an absent root is the identity — there is nothing to open.
    (
        "agentic/automation/src/runner.rs",
        "strip_prefix arithmetic only; never opens the path",
    ),
    // Serialised into a render context for templates to interpolate. Reading
    // it is the template's business, and the automation that does so is
    // already covered wherever it actually opens something.
    (
        "agentic/pipeline/src/automation_run.rs",
        "serialises the path into a JSON render context; opens nothing",
    ),
    // The trait's own default impl, overridden by the real host. An empty
    // fallback root is inert: `ContextRoot::fs(\"\")` resolves no globs, the
    // same outcome as the absent directory it stands in for.
    (
        "agentic/automation/src/workspace.rs",
        "default context_root impl + the port's own doc comments",
    ),
    // Listing handler on an IdeOnly route; `unwrap_or_default` feeds
    // strip_prefix for display, and route classification is what keeps it on a
    // node with files.
    (
        "agentic/http/src/routes/airway.rs",
        "IdeOnly listing handler; strip_prefix for display only",
    ),
    // ---- KNOWN VIOLATION, deliberately deferred. This is the backlog half. ----
    //
    // `export` and `cache` steps DO open (in fact create) what they resolve,
    // and on a worker that means `create_dir_all` succeeds against a phantom
    // root: the workspace directory is materialised on pod-ephemeral storage
    // and a file is written that nobody will ever read. Silent, and it trips
    // the compile boundary's rule that nothing on a resolution path may create
    // a directory.
    //
    // Not fixed here because the fix is a product decision, not a filter: these
    // steps predate custom apps and `ctx.storage`, so "write into the workspace
    // working copy" may have no fleet-safe meaning at all. The options are
    // retire / fail-fast / route to object storage, and picking one is not a
    // review this PR should carry.
    //
    // Adding `.filter(|p| p.is_dir())` here would be a one-line improvement
    // (fail instead of phantom-write) and is the obvious stopgap if the
    // decision stalls.
    (
        "agentic/automation/src/export.rs",
        "KNOWN VIOLATION — phantom write on a volume-less node; deferred pending a decision \
         on whether workspace-directory exports should exist at all",
    ),
];

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            // `target/` is build output; `tests/` includes this file, whose
            // prose necessarily contains the needle.
            if name == "target" || name == "tests" || name.starts_with('.') {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Lines that are prose rather than code. A doc comment explaining the trap
/// must not trip the test that enforces it.
fn is_prose(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("//") || t.starts_with("///") || t.starts_with("//!") || t.starts_with('*')
}

#[test]
fn workspace_path_is_filtered_by_is_dir_outside_the_allowlist() {
    // The agentic crates only: these are the task-execution paths that actually
    // run on the worker fleet. `oxy-app`'s handlers use a different method and
    // are governed by route classification.
    let crates_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/app has a parent");
    let agentic = crates_root.join("agentic");

    let mut files = Vec::new();
    rust_sources(&agentic, &mut files);
    // A floor, not a target. Its only job is to fail loudly if the walk breaks:
    // a boundary test that silently scans nothing is worse than no test.
    assert!(
        files.len() > 50,
        "expected to scan the agentic crates, found only {} files — the walk is broken",
        files.len()
    );

    let rel_of = |p: &Path| {
        p.strip_prefix(crates_root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/")
    };

    let mut violations = Vec::new();
    for path in &files {
        let rel = rel_of(path);
        if ALLOWED.iter().any(|(allowed, _)| rel.starts_with(allowed)) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !line.contains(NEEDLE) || is_prose(line) {
                continue;
            }
            // The chain may be split across lines by rustfmt, so look ahead.
            let window = lines[i..(i + 1 + LOOKAHEAD).min(lines.len())].join("\n");
            if SANCTIONED.iter().any(|ok| window.contains(ok)) {
                continue;
            }
            violations.push(format!("{rel}:{}: {}", i + 1, line.trim()));
        }
    }

    assert!(
        violations.is_empty(),
        "`workspace_path()` used without `.filter(|p| p.is_dir())`:\n\n{}\n\n\
         `Some` means the manager DECLARES a working copy, not that this node HAS one — it \
         is `Some` on a serve/worker replica with no volume, naming a directory that is not \
         there. Add `.filter(|p| p.is_dir())` so the guard fires on the node it was written \
         for, or add the file to ALLOWED with a reason if it genuinely never opens the path.",
        violations.join("\n")
    );
}

/// The allowlist must not rot. An entry naming a path that no longer exists is
/// a stale exemption that would silently cover a future file at that path.
#[test]
fn every_allowlist_entry_still_names_a_real_file() {
    let crates_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/app has a parent");
    for (rel, why) in ALLOWED {
        assert!(
            crates_root.join(rel).exists(),
            "allowlist entry {rel:?} ({why}) no longer exists — remove it rather than \
             leaving an exemption that would cover a future file at that path"
        );
    }
}

/// The test must be able to fail. A needle that no longer matches the codebase
/// would make this pass vacuously forever.
#[test]
fn the_needle_still_matches_the_sanctioned_form() {
    let crates_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/app has a parent");
    let known_good = crates_root.join("agentic/pipeline/src/pipeline_ref.rs");
    let text = fs::read_to_string(&known_good).expect("pipeline_ref.rs is readable");
    assert!(
        text.contains(NEEDLE),
        "NEEDLE {NEEDLE:?} no longer appears in pipeline_ref.rs — if the method was renamed, \
         this test is now scanning for nothing and passing vacuously"
    );
    assert!(
        text.contains(SANCTIONED[0]),
        "the sanctioned filter {:?} no longer appears in pipeline_ref.rs — if the idiom \
         changed (e.g. rustfmt now splits the call across lines), this test would report \
         every correct call site as a violation",
        SANCTIONED[0]
    );
}
