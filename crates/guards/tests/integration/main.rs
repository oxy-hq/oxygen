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
//! walked oxy-app's own `src` (and key their rows `../<crate>/src/…`) take
//! [`guards_root`]. Their oxy-app self-roots went with the crate in decompose
//! DEL (2026-11-25); none of them walks this crate's empty `src`, which would
//! pass by finding nothing.

use std::path::PathBuf;

mod app_scope_boundary;
mod authz_boundaries;
mod caller_has_pinned_constructors;
mod compiled_reader_is_not_a_back_door;
mod config_write_routes;
mod document_write_guards;
mod env_reads;
mod gated_caller_has_one_constructor;
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

/// `crates/guards`, the anchor the scanners key their walks and rows from:
/// every row reads `../<crate>/src/…`, as it did from `crates/app` before
/// decompose DEL deleted that crate (2026-11-25).
///
/// Never join `src` onto this: `crates/guards/src` is an empty `lib.rs`, and a
/// walk rooted there passes by scanning nothing. Fails closed if the anchor
/// stops being a sibling of the crates the scanners walk.
fn guards_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert!(
        root.join("../server/src").is_dir() && root.join("../app-core/src").is_dir(),
        "{} is no longer beside crates/server and crates/app-core: the scanners \
         rooted on it would pass by walking nothing. Repoint them.",
        root.display()
    );
    root
}
