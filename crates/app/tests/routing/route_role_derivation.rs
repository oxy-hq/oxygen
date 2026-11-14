//! Every handler that asks for a working copy must be on an IdeOnly route.
//!
//! `role_manifest.rs` is a hand-written list, and its failure mode is silence:
//! an unlisted route defaults to `FleetOk` and is served locally on a replica
//! that has no working copy. That is how `GET /apps/source/<file>` shipped —
//! the handler read `workspace_path()`, nobody added the entry, and it 404'd
//! only on the fleet. Nothing in the file catches that, because the file cannot
//! know about a route it does not mention.
//!
//! This derives the answer from the handler instead. A handler taking
//! `WorkspaceManagerWorkingCopy` has asked for `WorkspaceManager<WorkingCopy>`;
//! if its route is not IdeOnly, either the route is wrong or the handler should
//! be taking `WorkspaceManagerReadOnly`. Both are worth a build failure.
//!
//! **Deriving is not a substitute for declaring.** Measured back when the path
//! table still existed, a derived rule agreed with 62 of its 92 entries and
//! disagreed on 30 — which is why roles are now declared at the mount
//! (`route_ide` / `route_fleet`) rather than derived, and why this file only
//! guards the derivation's one sound direction. It fails OPEN on the whole
//! `agentic-http` surface, which sits below
//! `oxy-app` in the layering and structurally cannot name the extractor, so
//! `POST /analytics/runs` would silently become `FleetOk`. The derivation is
//! sound in one direction only, and that is the direction asserted here:
//!
//!   asks for a working copy  ⇒  must be IdeOnly        (checked)
//!   is IdeOnly               ⇒  asks for a working copy (NOT checked — false)
//!
//! The second implication is false for good reasons: `/events` correctly
//! declares it needs no disk and is pinned because it subscribes to a
//! process-local broadcaster, and `/ide` has no handler at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Routes whose handler takes the extractor but which are deliberately not
/// IdeOnly. Each needs a reason, because each is a place the derivation is
/// being overridden by hand.
const FLEET_OK_DESPITE_EXTRACTOR: &[(&str, &str)] = &[
    // Empty since the Factory retirement's step B (2026-11-14): the last
    // entry, `/databases`, stopped asking for a working copy when the
    // connection test it covered was deleted. A new entry needs a reason.
];

fn api_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/server/api")
}

/// Every `.rs` file under `src/server/api`, keyed by its path relative to that
/// directory (`admin/apps/handlers.rs`), never by its file name.
///
/// It was keyed by file stem, and this tree reuses names freely: `mod.rs`,
/// `handlers.rs`, `dto.rs`, and two `app.rs`. Each collision kept whichever
/// file `read_dir` reached last, and that order belongs to the filesystem, so
/// one tree passed on one CI runner and failed on another. The failure that
/// surfaced it: `api/admin/workspace_health/smoke/probes/app.rs` overwrote
/// `api/app.rs`, every `/apps` handler vanished, and `/apps` read as a stale
/// override. The same loss hides handlers from the guard below, which fails
/// OPEN on a body it never sees.
///
/// Directories are visited in sorted order so the walk is deterministic as
/// well, though with path keys the result no longer depends on it.
fn api_sources() -> BTreeMap<String, String> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
            .map(|entry| entry.expect("a directory entry").path())
            .collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                walk(root, &path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let body = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
                let key = path
                    .strip_prefix(root)
                    .expect("the walk starts at the root")
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(key, body);
            }
        }
    }
    let root = api_root();
    let mut out = BTreeMap::new();
    walk(&root, &root, &mut out);
    out
}

/// `(route pattern, handler fn name)` for every `.route("…", verb(path::to::fn))`
/// in the workspace router, with the nest prefix already applied.
fn mounted_routes() -> Vec<(String, String)> {
    // Source-file-relative, unlike every other path in this file — those join
    // `CARGO_MANIFEST_DIR`. It gained a `../` when this moved from a top-level
    // `tests/*.rs` into `tests/routing/`.
    let src = include_str!("../../src/server/router/workspace.rs");
    let mut nest_of: BTreeMap<String, String> = BTreeMap::new();
    for seg in src
        .split(".nest(")
        .skip(1)
        .chain(src.split(".nest_all(").skip(1))
        .chain(src.split(".nest_declared(").skip(1))
    {
        let Some(after) = seg.trim_start().strip_prefix('"') else {
            continue;
        };
        let Some(close) = after.find('"') else {
            continue;
        };
        let prefix = &after[..close];
        let Some(builder) = after[close..]
            .split_once("build_")
            .and_then(|(_, r)| r.split(['(', ' ', ')']).next())
        else {
            continue;
        };
        nest_of.insert(format!("build_{builder}"), prefix.to_string());
    }
    // Routes mounted DIRECTLY on `build_workspace_routes` carry no nest prefix
    // and are reachable through no builder. Omitting them left this guard
    // covering only the nested subset while still passing its own
    // "did the parser break" floor — which is the shape it exists to catch.
    nest_of.insert("build_workspace_routes".to_string(), String::new());

    let mut out = Vec::new();
    for (builder, prefix) in &nest_of {
        let Some(start) = src.find(&format!("fn {builder}")) else {
            continue;
        };
        let rest = &src[start..];
        let end = rest[1..].find("\nfn ").map(|i| i + 1).unwrap_or(rest.len());
        // Every mount form takes the path first, so a position scan needs only
        // the names. Collect by POSITION: splitting on one marker lets its
        // segment run through the next call and steal that route's handlers,
        // which reads as the wrong route touching disk.
        let body = &rest[..end];
        let mut mounts: Vec<(usize, usize)> = Vec::new();
        for marker in [
            ".route_ide(",
            ".route_fleet(",
            ".route_fleet_optional_working_copy(",
            ".route_split(",
            ".route_split_optional_working_copy(",
        ] {
            let mut from = 0;
            while let Some(i) = body[from..].find(marker) {
                let at = from + i;
                mounts.push((at, marker.len()));
                from = at + marker.len();
            }
        }
        mounts.sort_unstable();

        for (index, (at, head)) in mounts.iter().enumerate() {
            let stop = mounts.get(index + 1).map_or(body.len(), |(next, _)| *next);
            let seg = &body[at + head..stop];
            let Some(after) = seg.trim_start().strip_prefix('"') else {
                continue;
            };
            let Some(close) = after.find('"') else {
                continue;
            };
            let route = &after[..close];
            // `get(module::handler)` / `.post(module::handler)` — take every
            // handler on the route, since a route can carry several verbs.
            for call in after[close..].split("::").skip(1) {
                let name: String = call
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if name.is_empty() {
                    continue;
                }
                let full = if route == "/" {
                    prefix.clone()
                } else {
                    format!("{prefix}{route}")
                };
                out.push((full, name));
            }
        }
    }
    out
}

/// Does this handler ask for a `WorkspaceManager<WorkingCopy>`?
fn asks_for_a_working_copy(sources: &BTreeMap<String, String>, handler: &str) -> bool {
    asks_for_extractor(sources, handler, "WorkspaceManagerWorkingCopy")
}

/// Does this handler's signature name `extractor`?
fn asks_for_extractor(sources: &BTreeMap<String, String>, handler: &str, extractor: &str) -> bool {
    sources.values().any(|body| {
        let Some(at) = body.find(&format!("fn {handler}(")) else {
            return false;
        };
        // The signature ends at the return arrow or the opening brace.
        let tail = &body[at..];
        let end = tail
            .find(") ->")
            .or_else(|| tail.find(") {"))
            .unwrap_or(tail.len().min(600));
        tail[..end].contains(extractor)
    })
}

/// The one direction the derivation is sound in.
#[test]
fn a_handler_that_asks_for_a_working_copy_is_on_an_ide_only_route() {
    oxy_app::server::role_manifest::install_route_declarations_for_tests();
    let sources = api_sources();
    let mut offenders: Vec<String> = Vec::new();
    let mut checked = 0;
    let mut read_only = 0;

    for (route, handler) in mounted_routes() {
        if asks_for_extractor(&sources, &handler, "WorkspaceManagerReadOnly") {
            read_only += 1;
        }
        if !asks_for_a_working_copy(&sources, &handler) {
            continue;
        }
        checked += 1;
        if FLEET_OK_DESPITE_EXTRACTOR
            .iter()
            .any(|(r, _)| route.starts_with(r))
        {
            continue;
        }
        let concrete = route
            .replace("{pathb64}", "cGF0aA")
            .replace("{*rest}", "x")
            .replace("{file_path}", "f.json")
            .replace("{chart_path}", "c.png");
        let concrete: String = concrete
            .split('/')
            .map(|s| {
                if s.starts_with('{') {
                    "x".to_string()
                } else {
                    s.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("/");
        let full = format!("/api/d9830be4-c6a4{concrete}");
        let ide_only = ["GET", "POST", "PUT", "DELETE", "PATCH"].iter().any(|m| {
            oxy_app::server::role_manifest::classify(m, &full)
                == oxy_app::server::role_manifest::RouteRole::IdeOnly
        });
        if !ide_only {
            offenders.push(format!("{route}  (handler `{handler}`)"));
        }
    }

    assert!(
        // No mounted handler asks for a working copy any more (step B of the
        // Factory retirement, 2026-11-14), so `checked` is legitimately 0.
        // What proves the parser still works is that it finds the handlers
        // that take the read-only manager: 22 files use it today.
        read_only >= 10,
        "only {read_only} mounted handlers take `WorkspaceManagerReadOnly` \
         ({checked} take a working copy) — the parser broke; fix it rather \
         than letting the guard pass vacuously",
    );
    offenders.sort();
    offenders.dedup();
    assert!(
        offenders.is_empty(),
        "these handlers ask for a working copy but their route is not IdeOnly:\n  {}\n\n\
         This is the gap `role_manifest.rs` cannot see: an unlisted route defaults \
         to FleetOk and 404s only on the fleet. Two fixes, and the first is usually \
         right:\n\
         \x20 1. The handler does not actually need a disk — take \
         `WorkspaceManagerReadOnly` and let the compiler confirm it.\n\
         \x20 2. It does — mount the route with `route_ide`, not `route_fleet`.\n\
         If neither (an optional, degrading read like `/databases`), add it to \
         FLEET_OK_DESPITE_EXTRACTOR with the reason.",
        offenders.join("\n  ")
    );
}

/// The override list shrinks when a handler stops asking.
#[test]
fn the_override_list_has_no_stale_entries() {
    let sources = api_sources();
    let mounted = mounted_routes();
    let mut stale: Vec<&str> = Vec::new();
    for (route, _) in FLEET_OK_DESPITE_EXTRACTOR {
        let still = mounted
            .iter()
            .any(|(r, h)| r.starts_with(route) && asks_for_a_working_copy(&sources, h));
        if !still {
            stale.push(route);
        }
    }
    assert!(
        stale.is_empty(),
        "these are overridden but no longer ask for a working copy — remove them:\n  {}",
        stale.join("\n  ")
    );
}

/// `api_sources` keeps one entry per FILE, not one per file name.
///
/// Keyed by name it dropped every file but one per name, silently, in an order
/// the filesystem chose, which is how one tree went green on the PR and red on
/// `main`. The count comes from a second walk that keys nothing, so any
/// overwrite in `api_sources` shows up here as a shortfall, in every order.
#[test]
fn api_sources_keeps_every_file_even_when_names_repeat() {
    let sources = api_sources();

    let mut on_disk = 0;
    let mut stack = vec![api_root()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read an api directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                on_disk += 1;
            }
        }
    }
    assert_eq!(
        sources.len(),
        on_disk,
        "api_sources holds {} entries for {on_disk} .rs files: files that share a \
         key overwrote each other, and the handlers in the lost ones are invisible \
         to both guards above",
        sources.len(),
    );

    // The collision that turned `main` red, named so a regression says which
    // handlers went missing rather than only that a count is off.
    let thread = sources
        .get("thread.rs")
        .expect("api/thread.rs is where the router's `thread::` handlers live");
    assert!(
        thread.contains("fn get_threads("),
        "the `thread.rs` entry is not api/thread.rs, so every `/threads` handler is missing",
    );
}
