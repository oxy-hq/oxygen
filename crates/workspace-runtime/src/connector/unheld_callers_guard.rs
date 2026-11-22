//! Only `OxyProjectContext` may reach a connector, config or credential that
//! skips the preview write hold.
//!
//! These functions were `pub` from W1b until W4 (both 2026-11-22), only
//! because `OxyProjectContext`, then still in `oxy-app`, called them and wraps
//! the result in the hold itself. W4 moved it down beside them, so they are
//! `pub(crate)` again and the compiler keeps every other crate out. This scan
//! still pins them inside this crate: the rest of the agentic wiring (the
//! preview runtime) is moving in too, and a module here could otherwise hand a
//! workspace-preview request a connector that writes. It fails if any file
//! outside the two allowed places names one.

use std::path::{Path, PathBuf};

/// The functions that bypass the hold.
const UNHELD: &[&str] = &[
    "build_unheld_connector",
    "resolve_connector_impl",
    "resolve_pre_built_airhouse",
    "airhouse_wire_params",
];

/// Paths (relative to the repository's `crates/`) that may name them: this
/// module, and `project_ctx`, where `OxyProjectContext` lives (`oxy-app`'s
/// `agentic_wiring/project_ctx/` until W4).
const ALLOWED: &[&str] = &[
    "workspace-runtime/src/connector.rs",
    "workspace-runtime/src/connector/",
    "workspace-runtime/src/project_ctx.rs",
    "workspace-runtime/src/project_ctx/",
];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name == "node_modules" || name.starts_with('.') {
                continue;
            }
            rust_files(&path, out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

/// Whether `line` names `ident` as a whole identifier.
fn names(line: &str, ident: &str) -> bool {
    line.match_indices(ident).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + ident.len()..].chars().next();
        let is_ident = |c: char| c.is_alphanumeric() || c == '_';
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

#[test]
fn only_the_project_context_names_an_unheld_connector_door() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/")
        .to_path_buf();
    let mut files = Vec::new();
    rust_files(&crates, &mut files);
    assert!(
        files.len() > 100,
        "scanned only {} files under {}: the walk is broken",
        files.len(),
        crates.display()
    );

    let mut offenders = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&crates)
            .expect("under crates/")
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.iter().any(|allowed| rel.starts_with(allowed)) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            for ident in UNHELD {
                if names(line, ident) {
                    offenders.push(format!("crates/{rel}:{}: {ident}", n + 1));
                }
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these bypass the preview write hold, so only OxyProjectContext may call \
         them (use `build_connector_for_db`, which holds):\n{}",
        offenders.join("\n")
    );
}

#[test]
fn the_scan_sees_the_allowed_caller() {
    // Fails open otherwise: if `project_ctx` stopped naming these (or moved),
    // the allowlist would be pinning nothing.
    let caller = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/project_ctx/project_context.rs");
    let text = std::fs::read_to_string(&caller).expect("project_context.rs");
    assert!(names(&text, "resolve_pre_built_airhouse"));
    assert!(!names("build_unheld_connector_x", "build_unheld_connector"));
}
