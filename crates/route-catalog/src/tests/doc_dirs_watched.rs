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
/// in it), and this test walked it again in the slot it held and required the
/// same catalog, so a doc only it supplied could not vanish from `oxyc routes`
/// silently. Decompose DEL deleted the crate (2026-11-25), so that comparison
/// has nothing left to walk and went with it. What stays is the half that
/// still means something: neither list the build script reads may name the
/// deleted tree, and every dir they name must exist, since a dir that is not
/// there is walked as empty and its routes and docs drop out without an error.
#[test]
fn dropping_the_oxy_app_tree_changes_no_route() {
    const APP_TREE: &str = "crates/app";
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for (list, dirs) in [
        ("DOC_DIRS", walker::DOC_DIRS),
        ("SOURCE_DIRS", walker::SOURCE_DIRS),
    ] {
        assert!(!dirs.is_empty(), "{list} is empty");
        for dir in dirs {
            assert!(
                *dir != APP_TREE && !dir.starts_with("crates/app/"),
                "{list} names `{dir}`, inside the deleted oxy-app tree; it left \
                 DOC_DIRS in decompose Z0 and the workspace in decompose DEL"
            );
            assert!(
                root.join(dir).is_dir(),
                "{list} names `{dir}`, which is not a directory: the build \
                 script would walk it as empty and drop its routes silently"
            );
        }
    }
}
