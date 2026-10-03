//! Boundary test: a file that mutates an `organizations` or `apps` row must
//! say what that does to the custom-app resolution cache.
//!
//! ## Why a source-scanning test
//!
//! `custom_apps_cache::cached_app_resolution` is keyed `(org_slug, app_slug)`
//! and caches the two rows behind it for `CACHE_TTL`. That makes an org slug,
//! an app slug, `published_at`, the channel pointers, `visibility`, and the
//! existence of either row all *cache state* — mutated from handlers that live
//! nowhere near the custom-apps code and have no reason to know the cache
//! exists.
//!
//! This is not hypothetical. The cache shipped with invalidation on the app
//! mutation sites, and review then found three misses in a row: `update_app`
//! and `delete_one` (app slug rename / delete), then `admin::orgs_admin::
//! rename_org`, then the tenant-facing `organizations::update_org` — which is
//! the *common* rename path, an org admin renaming their own org. Each fix was
//! correct and each left the next one uncovered, because remembering to call an
//! invalidator is exactly the kind of obligation a reviewer cannot see the
//! absence of.
//!
//! So the objection has to be mechanical: if a file writes one of these rows,
//! it must mention the invalidator.
//!
//! ## Known gaps — this is file-level, and the write list is a known list
//!
//! A file that already invalidates somewhere passes, even if a *new* handler in
//! it forgets. `admin/orgs_admin.rs` was exactly that shape: `rename_org`
//! invalidated, `delete_org` did not, and this test would not have caught it.
//!
//! Detection is also string matching over [`ROW_WRITES`], so it sees the write
//! spellings that exist today (`ActiveModel` plus the entity-level
//! `update_many` / `delete_many` / `delete_by_id`). A raw SQL `UPDATE`, or a
//! Sea-ORM verb nobody has used yet, is invisible until added there.
//!
//! That is a deliberate trade, not an oversight. Call-level attribution needs
//! real parsing, and the failure mode of a fuzzy source-scanner is a false
//! positive — which gets the test deleted, after which it protects nothing.
//! File-level with zero false positives catches the larger shape (a whole
//! handler module that never learned the cache exists) and is honest about the
//! rest.
//!
//! ## The allowlist is for files that genuinely cannot invalidate anything
//!
//! Only two shapes belong: writes that never touch the cache key or a cached
//! field, and row *creation* (a miss is never cached, so a new org or app is
//! reachable immediately with no invalidation). Anything else is a real
//! obligation — add the call, don't add the entry.

use std::fs;
use std::path::{Path, PathBuf};

/// Naming one of these means the file can change what `cached_app_resolution`
/// would return.
///
/// `ActiveModel` alone is not enough. Sea-ORM's entity-level verbs mutate the
/// same rows without ever constructing one, and three sites already do:
/// `Apps::update_many` (`org_teams/service.rs`), `Organizations::delete_by_id`
/// (`organizations/org_handlers.rs`) and `apps::Entity::delete_by_id`
/// (`custom_apps_publish.rs`). The suite was green over those only because each
/// of those files *also* names an `ActiveModel` or already invalidates — so a
/// new module whose sole mutation was an `update_many` would have passed
/// silently, which is precisely the shape this test exists to catch.
///
/// Both spellings of each entity are listed because both are in use: the
/// re-exported `Apps` / `Organizations` and the qualified `apps::Entity` /
/// `organizations::Entity`.
const ROW_WRITES: &[&str] = &[
    "organizations::ActiveModel",
    "apps::ActiveModel",
    "Apps::update_many",
    "Apps::delete_many",
    "Apps::delete_by_id",
    "Organizations::update_many",
    "Organizations::delete_many",
    "Organizations::delete_by_id",
    "apps::Entity::update_many",
    "apps::Entity::delete_many",
    "apps::Entity::delete_by_id",
    "organizations::Entity::update_many",
    "organizations::Entity::delete_many",
    "organizations::Entity::delete_by_id",
];

/// The call that discharges the obligation.
const INVALIDATOR: &str = "invalidate_app_resolution_cache";

/// Files that write one of the rows but cannot invalidate the cache, and why.
const ALLOWED: &[(&str, &str)] = &[
    (
        "server/api/org_logo.rs",
        "writes only `logo` / `logo_content_type` / `updated_at` — no slug, no \
         deletion, and no field the serve path reads from the cached row",
    ),
    (
        "api-tenancy/src/partner_console/orgs.rs",
        "creates orgs (a miss is never cached, so a new org needs no \
         invalidation) and renames only the display `name`, never the slug",
    ),
    (
        "airhouse/src/local_seed.rs",
        "creates the nil-UUID local-mode org only if it is missing — a miss is \
         never cached, and it never updates a slug or deletes a row",
    ),
    (
        "server/api/admin/airway_config/handlers_tests.rs",
        "test fixture: inserts an org per case under a fresh uuid slug so the \
         platform-scope cases have something to scope against. Inserts only — a \
         miss is never cached — and it never updates a slug nor deletes a row",
    ),
    (
        "server/api/admin/airway_config/preview_scan_tests.rs",
        "test fixture, same shape as its sibling above: inserts an org per case \
         under a fresh uuid slug so the preview's platform-scope cases have \
         something to scope against. Inserts only — a miss is never cached — and \
         it never updates a slug nor deletes a row",
    ),
];

/// Files below `oxy-app` that write a row but cannot name the invalidator,
/// because the cache lives above them — so they take the flush as a callback
/// and `oxy-app` wraps them. `(writer, wrapper, why)`.
///
/// Not an exemption: the writer is excused only while its WRAPPER calls the
/// invalidator, so the obligation moves to a file this test still checks
/// rather than disappearing. The writer's signature makes the callback
/// mandatory, so a caller cannot skip it by omission either.
const DELEGATED: &[(&str, &str, &str)] = &[(
    "tenancy/src/org_teams/service.rs",
    "app/src/server/api/org_teams/service.rs",
    "`write_access` updates `apps.visibility` and runs a caller-supplied \
     `on_committed`; oxy-app's 4-arg wrapper, which every surface calls, \
     passes `invalidate_app_resolution_cache`",
)];

/// The seed commands, which are the only row writers outside the server. They
/// build rows against a fresh database with no serving process attached, so
/// they hold no obligation — skipping them is cheaper than three allowlist
/// entries that all say the same thing.
///
/// Matched on the *relative path*, not the bare filename: a filename match
/// would silently exempt any future `seed_*.rs` anywhere in the crate, which
/// is a wider hole than this exemption is meant to be.
const SEED_PREFIX: &str = "app/src/cli/commands/seed";

/// `crates/…`-relative, forward-slashed — the shape [`SEED_PREFIX`] is written in.
///
/// Rooted at `crates/`, not `crates/app/src`, so a row-writer that moves into a
/// sibling crate stays in frame. `ALLOWED` is matched with `ends_with`, so its
/// entries are unaffected by the wider prefix.
fn relative_to_src(path: &Path) -> String {
    path.strip_prefix(crates_root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// `CARGO_MANIFEST_DIR` is `crates/app`; its parent is the `crates/` root.
fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/app has a parent")
        .to_path_buf()
}

/// Production sources only — see [`SEED_PREFIX`].
fn production_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            production_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            // Production sources only. Now that the walk starts at the `crates/` root it
            // would otherwise sweep in every crate's `tests/` tree. Mirrors
            // `authz_boundaries`.
            && path.components().any(|c| c.as_os_str() == "src")
            && !relative_to_src(&path).starts_with(SEED_PREFIX)
        {
            out.push(path);
        }
    }
}

#[test]
fn row_writers_acknowledge_the_resolution_cache() {
    // The whole crate, not just `server/api`: these rows are also reachable
    // from `server/service/` and from a CLI command, and a scan scoped to the
    // handlers would call those out of frame rather than covered.
    let src = crates_root();
    assert!(
        src.is_dir(),
        "expected {} to exist — did the crate layout move?",
        src.display()
    );

    let mut files = Vec::new();
    production_sources(&src, &mut files);
    // A scan that silently walks nothing (or only oxy-app, after the walk root was
    // widened) reports a false green — worse than no test at all. 500 is a floor,
    // not a target: it only has to stay below the real count (~1450 today), so a
    // future split that moves crates out of this workspace lowers it rather than
    // chasing it upward.
    assert!(
        files.len() > 500,
        "expected to scan every crate's sources, found only {} files — the walk is \
         broken, and a boundary test that silently scans nothing is worse than no test",
        files.len()
    );
    assert!(
        files
            .iter()
            .any(|f| !relative_to_src(f).starts_with("app/")),
        "walk covered only oxy-app — a sibling crate would escape this test"
    );

    let mut offenders = Vec::new();
    for path in &files {
        let Ok(source) = fs::read_to_string(path) else {
            continue;
        };
        let Some(needle) = ROW_WRITES.iter().find(|n| source.contains(**n)) else {
            continue;
        };
        if source.contains(INVALIDATOR) {
            continue;
        }
        let rel = relative_to_src(path);
        if ALLOWED.iter().any(|(allowed, _)| rel.ends_with(allowed)) {
            continue;
        }
        if let Some((_, wrapper, _)) = DELEGATED.iter().find(|(w, _, _)| rel == *w) {
            let wrapper_src = fs::read_to_string(crates_root().join(wrapper)).unwrap_or_default();
            if wrapper_src.contains(INVALIDATOR) {
                continue;
            }
            offenders.push(format!(
                "  {rel}  (writes `{needle}`; delegates to {wrapper}, which no longer calls it)"
            ));
            continue;
        }
        offenders.push(format!("  {rel}  (writes `{needle}`)"));
    }

    assert!(
        offenders.is_empty(),
        "These files write an `organizations` or `apps` row but never call \
         `{INVALIDATOR}`:\n{}\n\n\
         The custom-app serve path caches `(org_slug, app_slug) -> (org, app)` for \
         `CACHE_TTL`. A slug is the cache KEY, and `published_at`, the channel \
         pointers, `visibility` and the rows' existence are cached VALUES — so an \
         update or a delete here leaves the serve path answering from a row that \
         no longer exists.\n\n\
         Either call `custom_apps_cache::{INVALIDATOR}()` after the write, or — if \
         this file only CREATES rows (a miss is never cached) or writes fields the \
         serve path never reads — add it to ALLOWED in this file with the reason.",
        offenders.join("\n")
    );
}
