//! Workspace-wide guards: source and manifest scans across every crate.
//!
//! One binary for the whole crate; add a case as a `mod` here, never as a new
//! `tests/*.rs`. These moved out of `oxy-app`'s `authz`, `platform` and
//! `routing` groups in decompose GRD (2026-11-24). They read source text, so they
//! never needed oxy-app's dependency graph: this link carries no DuckDB,
//! DataFusion, Arrow or AWS SDK, and rebuilds only when a file here changes.
//! `oxy-guards` depends on nothing it scans, so graph-based test selection
//! never picks it; `.config/test-selection.toml` runs it on every PR
//! (`always-test`).
//!
//! **Ungrouped in `.config/nextest.toml`, deliberately.** Nothing here opens a
//! database, so the binary runs at full parallelism. `shared_db_registry` scans
//! this directory too and fails the build the day a case here starts reaching
//! the shared `OXY_DATABASE_URL`.
//!
//! Paths: `CARGO_MANIFEST_DIR` is `crates/guards`, so `..` is still `crates/`
//! and `../..` the repo root, exactly as from `crates/app`. The scanners that
//! walked oxy-app's own `src` (and key their rows relative to `crates/app`) take
//! [`app_root`] instead of `CARGO_MANIFEST_DIR`, so they keep walking oxy-app's
//! tree rather than this crate's empty `src`.

use std::path::PathBuf;

mod app_scope_boundary;
mod authz_boundaries;
mod compiled_reader_is_not_a_back_door;
mod config_write_routes;
mod document_write_guards;
mod env_reads;
mod handler_spawns;
mod no_dropped_partner_tables;
mod paginated_links_keep_the_api_prefix;
mod sentry_hub_scan;
mod shared_db_registry;
mod surface_contract;
mod undeclared_mounts_stay_diskless;
mod workspace_path_backdoor;
mod workspace_path_escape_hatch;
mod workspace_path_needs_is_dir;

/// `crates/app`, the root the oxy-app-relative scanners key their walks and
/// rows from (they ran with `CARGO_MANIFEST_DIR = crates/app` before GRD).
///
/// Fails closed: if oxy-app's `src` is gone, a walk rooted here would find
/// nothing and pass, so this panics instead. When decompose DEL deletes
/// `crates/app`, it deletes this and repoints every caller.
fn app_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../app");
    assert!(
        root.join("src").is_dir(),
        "{} has no src: oxy-app moved, and the scanners rooted on it would pass \
         by walking nothing. Repoint them.",
        root.display()
    );
    root
}

/// A directory under [`app_root`] that a scanner walks with a silent
/// `let Ok(..) = read_dir(..) else { return }`. Asserts it exists, so the day
/// a later PR empties it the walk fails instead of shrinking.
fn app_dir(rel: &str) -> PathBuf {
    let dir = app_root().join(rel);
    assert!(
        dir.is_dir(),
        "oxy-app's {rel} is gone: repoint the walk that reads it"
    );
    dir
}
