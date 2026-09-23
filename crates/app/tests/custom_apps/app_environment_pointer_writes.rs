//! Boundary test: a file that moves a build pointer on `apps` must mirror it into
//! `app_environments` (spec §3.1, Phase 1a).
//!
//! Same shape and the same honesty as `custom_apps_cache_invalidation.rs`. This is
//! file-level string matching over the mutation spelling every real write site uses
//! today (`active.<pointer> = ActiveValue::Set(`), so a raw SQL `UPDATE`, or a file
//! that mirrors in one function and forgets in another, is invisible to it. It
//! catches the larger shape: a new write path that never learned environments exist.

use std::fs;
use std::path::{Path, PathBuf};

const POINTER_WRITES: &[&str] = &[
    ".draft_build_id = ActiveValue::Set(",
    ".published_build_id = ActiveValue::Set(",
];

const MIRROR_CALL: &str = "custom_apps_environments::record_move(";

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Production sources only. Test directories and `tests.rs` files seed pointers on
/// purpose; `migration` and `entity` define the columns.
fn source_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read crates dir") {
        let path = entry.expect("dir entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if matches!(
                name,
                "tests" | "target" | "migration" | "entity" | "node_modules"
            ) {
                continue;
            }
            source_files(&path, out);
        } else if name.ends_with(".rs") && name != "tests.rs" {
            out.push(path);
        }
    }
}

fn pointer_writing_files() -> Vec<(PathBuf, String)> {
    let mut files = Vec::new();
    source_files(&crates_dir(), &mut files);
    files
        .into_iter()
        .filter_map(|path| {
            let src = fs::read_to_string(&path).ok()?;
            POINTER_WRITES
                .iter()
                .any(|w| src.contains(w))
                .then_some((path, src))
        })
        .collect()
}

#[test]
fn every_pointer_write_is_mirrored_into_app_environments() {
    let offenders: Vec<String> = pointer_writing_files()
        .into_iter()
        .filter(|(_, src)| !src.contains(MIRROR_CALL))
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "these files move a build pointer on `apps` without calling `{MIRROR_CALL}`:\n{}",
        offenders.join("\n")
    );
}

/// Guards the guard: if a refactor changes the write spelling, the test above
/// passes vacuously. Four files write pointers today: custom_apps_publish.rs,
/// admin/apps/handlers.rs, admin/apps/ops.rs and cli/commands/seed_apps.rs.
#[test]
fn the_scanner_still_sees_the_known_pointer_writers() {
    let writers = pointer_writing_files().len();
    assert!(
        writers >= 4,
        "expected at least 4 pointer-writing files, found {writers}"
    );
}
