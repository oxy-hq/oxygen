//! The build script reads handler docs from `oxy-server` only in the dirs
//! `DOC_DIRS` names, not the whole `crates/server/src`, so an edit elsewhere in
//! `oxy-server` does not re-run it (decompose P1: a `drivers/` edit paid ~2 s of
//! route-catalog in series). That is only safe while no route's description
//! comes from an `oxy-server` file outside those dirs: such a doc would be
//! missing from the catalog, or go stale, with nothing re-running the script.
//!
//! So this walks the sources twice, once with `DOC_DIRS` and once with the
//! whole of `crates/server/src` in their place, and requires the same catalog.

// The build script's walker, std-only, compiled here as well as in `build.rs`.
#[allow(dead_code)]
#[path = "../../build_route_catalog.rs"]
mod walker;

const SERVER_SRC: &str = "crates/server/src";

#[test]
fn no_handler_doc_lives_in_an_unwatched_oxy_server_file() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !root.join(SERVER_SRC).is_dir() {
        return;
    }
    // The whole crate, in the slot its narrowed dirs occupy: `collect_dir`
    // walks sorted, so this is the doc pool's order before the narrowing.
    let mut wide: Vec<&str> = Vec::new();
    for dir in walker::DOC_DIRS {
        if !dir.starts_with(SERVER_SRC) {
            wide.push(dir);
        } else if !wide.contains(&SERVER_SRC) {
            wide.push(SERVER_SRC);
        }
    }
    assert!(
        wide.contains(&SERVER_SRC),
        "DOC_DIRS names no `{SERVER_SRC}` dir; update this test with it"
    );

    let key = |r: &walker::Route| {
        (
            r.surface,
            r.path.clone(),
            r.method.clone(),
            r.description.clone(),
        )
    };
    let narrow: Vec<_> = walker::collect(&root, walker::DOC_DIRS)
        .iter()
        .map(key)
        .collect();
    let full: Vec<_> = walker::collect(&root, &wide).iter().map(key).collect();
    let missing: Vec<_> = full.iter().filter(|r| !narrow.contains(r)).collect();
    assert!(
        missing.is_empty() && narrow.len() == full.len(),
        "these routes take their description from an oxy-server file outside the \
         `{SERVER_SRC}/*` dirs DOC_DIRS names, so editing that file would not \
         re-run the route-catalog build script: {missing:#?}\n\
         Add the handler's dir to DOC_DIRS in crates/route-catalog/build_route_catalog.rs."
    );
}

/// `oxy-app`'s tree left `DOC_DIRS` in decompose Z0 (no handler doc was left
/// in it). While the crate still exists, walk it again in the slot it held,
/// first, and require the same catalog: a doc that only it supplied, or one
/// that won a tie there, would otherwise vanish from `oxyc routes` silently.
/// Goes vacuous with the crate itself (decompose DEL).
#[test]
fn dropping_the_oxy_app_tree_changes_no_route() {
    const APP_SRC: &str = "crates/app/src";
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if !root.join(APP_SRC).is_dir() {
        return;
    }
    assert!(
        !walker::DOC_DIRS.contains(&APP_SRC),
        "DOC_DIRS names `{APP_SRC}` again; it left in decompose Z0"
    );
    let mut with_app = vec![APP_SRC];
    with_app.extend_from_slice(walker::DOC_DIRS);

    // Every field `build.rs` writes into the served table.
    let key = |r: &walker::Route| {
        (
            r.method.clone(),
            r.path.clone(),
            r.handler.clone(),
            r.surface,
            r.description.clone(),
            r.note.clone(),
        )
    };
    let now: Vec<_> = walker::collect(&root, walker::DOC_DIRS)
        .iter()
        .map(key)
        .collect();
    let before: Vec<_> = walker::collect(&root, &with_app).iter().map(key).collect();
    assert_eq!(
        now, before,
        "walking `{APP_SRC}` changes the catalog, so a route took its description \
         from oxy-app; move that handler's doc to the crate that owns it"
    );
}
