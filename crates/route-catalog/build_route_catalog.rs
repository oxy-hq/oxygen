//! Build-time extraction of the Oxy HTTP route table.
//!
//! Axum exposes no route table at runtime (see the note on
//! `every_workspace_mount_is_classified` in `server/role_manifest.rs`), so the
//! only way to hand `oxyc routes` a *complete* and *never-stale* list of
//! endpoints is to read it out of the router source at compile time.
//!
//! This walks the router builders the same way axum composes them — following
//! `.route(...)`, `.nest(...)` and `.merge(...)` from a small set of seed
//! entry points — and emits `$OUT_DIR/route_catalog_generated.rs`.
//!
//! It is deliberately a *lexical* walker, not a Rust parser: the router
//! modules are flat builder chains, and a `syn` dependency in the build graph
//! of the workspace's largest crate is not worth the extra fidelity.
//!
//! The tradeoff is that a call it cannot resolve is **dropped, not guessed** —
//! so a missing route means the walker lost the thread, never that the route
//! does not exist. `server::route_catalog`'s tests are the guard: they fail if
//! the walk stops reaching a surface or a landmark route.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// One resolved endpoint.
pub struct Route {
    pub method: String,
    pub path: String,
    pub handler: String,
    pub surface: &'static str,
    /// Which [`SOURCE_DIRS`] entry the mounting builder was found in. Not
    /// emitted into the catalog — it exists so the build can prove every
    /// scanned tree actually contributes, which is what catches a tree added
    /// to `SOURCE_DIRS` whose seed nobody wired up.
    pub source_dir: &'static str,
    /// The handler's own doc comment, flattened to one line — what the
    /// endpoint does. Empty when the handler carries none.
    pub description: String,
    /// The comment the router source carries above (or inside) this mount,
    /// flattened to one line — usually *why* the route sits where it does.
    /// Empty when the mount has none.
    pub note: String,
}

/// A router builder function found in the source tree.
struct RouterFn {
    /// Function name, e.g. `build_workspace_routes`.
    name: String,
    /// Module path segments, e.g. `["oxy_app", "server", "router", "workspace"]`.
    module: Vec<String>,
    /// Comment-stripped function body (including the outer braces).
    body: String,
    /// `ident -> full use path` for the file the function lives in.
    imports: HashMap<String, Vec<String>>,
    /// Modules this file glob-imports (`use handlers::*`). A glob brings names
    /// into scope without naming them, so an unqualified handler can be defined
    /// in any of these — `oxy-api-tenancy`'s `onboarding` module mounts
    /// `post(setup_demo)` on the strength of one.
    glob_imports: Vec<GlobImport>,
    /// Directory of the defining file, used to break `(module, fn)` ties.
    dir: PathBuf,
    /// The defining file itself, for resolving unqualified handler names.
    file: PathBuf,
    /// The [`SOURCE_DIRS`] entry this builder was indexed from; empty for a
    /// function found during the doc-only pass.
    source_dir: &'static str,
    /// `(route path literal, METHOD) -> the comment that documents it`,
    /// harvested from the un-stripped body.
    ///
    /// Keyed per function, so the same literal in two builders keeps its own
    /// note; and by method, because one builder routinely mounts the same
    /// literal several times (`build_thread_routes` has three `.route("/", …)`)
    /// and a comment written above the destructive DELETE must not end up on
    /// the GET. `None` marks a key that appeared twice — better no note than
    /// one belonging to a different endpoint.
    docs: HashMap<(String, String), Option<String>>,
}

/// One `use …::*` in a file.
#[derive(Clone)]
struct GlobImport {
    /// Module path with any `crate`/`self`/`super` prefix stripped, so it can
    /// be suffix-matched against an indexed module path.
    path: Vec<String>,
    /// Whether the import resolves against the file's own module tree rather
    /// than the crate root. `self::` and `super::` are relative by definition;
    /// `crate::` is absolute.
    ///
    /// A **bare** path is genuinely ambiguous — `use handlers::*` beside a
    /// `mod handlers;` is a local module, while `use entity::prelude::*` is an
    /// external crate (Rust itself errors on the ambiguous case). It is
    /// classified relative, which keeps the tighter gate: the local-module
    /// shape is the one that supplies handler docs, and treating an
    /// extern-crate glob as relative only costs a description that was never
    /// going to be a handler's.
    relative: bool,
}

/// A documented function anywhere in the scanned sources, so a route can
/// report what its handler says about itself.
struct FnDoc {
    module: Vec<String>,
    name: String,
    doc: String,
    /// The file it is defined in. An *unqualified* handler name can only be
    /// resolved against the file that mounts it — matching on the bare name
    /// alone picks up any `list()` in the tree.
    file: PathBuf,
}

/// Where a walk starts: a label for grouping, the URL prefix axum mounts the
/// builder under, and the `module::function` that builds it.
struct Seed {
    surface: &'static str,
    prefix: &'static str,
    module: &'static str,
    function: &'static str,
}

/// Roots of the live HTTP surface, mirroring `server/router/entry.rs`.
///
/// `build_public_routes` / `build_global_routes` / `build_protected_routes` are
/// merged into one router that `cli/commands/serve.rs` nests under `/api`;
/// `build_external_api_router` mounts the curated API-key surface under
/// `/external/api`. The sibling API crates arrive through three seams the
/// `oxy-server` composition root fills: `SurfaceSeams::api`, merged beside the
/// org tree at `/api`, `SurfaceSeams::workspace`, merged *inside* the
/// `/{workspace_id}` tree, and `SurfaceSeams::public`, merged beside the public
/// tree. A crate can use more than one — `oxy-api-tenancy` and
/// `oxy-api-frontline` do, which is why each has two seeds.
const SEEDS: &[Seed] = &[
    Seed {
        surface: "public",
        prefix: "/api",
        module: "public",
        function: "build_public_routes",
    },
    // Route discovery itself. Its own seed because `build_protected_routes`
    // (where it is merged) is deliberately NOT one — that function composes the
    // global and workspace trees, which are seeded separately, so seeding it
    // too would double-count every route beneath it. Without this entry the
    // one endpoint missing from the endpoint list would be the endpoint that
    // serves the list.
    Seed {
        surface: "org",
        prefix: "/api",
        module: "protected",
        function: "build_catalog_routes",
    },
    Seed {
        surface: "org",
        prefix: "/api",
        module: "global",
        function: "build_global_routes",
    },
    Seed {
        surface: "workspace",
        prefix: "/api/{workspace_id}",
        module: "workspace",
        function: "build_workspace_routes",
    },
    // Mounted beside the workspace tree, not in it (its own access middleware),
    // so it is not reached by the walk above.
    Seed {
        surface: "workspace",
        prefix: "/api/{workspace_id}/previews",
        module: "workspace",
        function: "build_workspace_preview_routes",
    },
    Seed {
        surface: "cameras",
        prefix: "/api",
        module: "oxy_cameras::routes",
        function: "router",
    },
    Seed {
        surface: "org",
        prefix: "/api",
        module: "oxy_api_github",
        function: "routes",
    },
    Seed {
        surface: "org",
        prefix: "/api",
        module: "oxy_api_tenancy::partner_console",
        function: "routes",
    },
    Seed {
        surface: "org",
        prefix: "/api",
        module: "oxy_api_tenancy::onboarding",
        function: "routes",
    },
    Seed {
        surface: "org",
        prefix: "/api",
        module: "oxy_api_documents::router",
        function: "routes",
    },
    // `oxy-api-frontline` fills two seams: its org-admin routes ride
    // `SurfaceSeams::api` like the crates above, and its PIN sign-in + kiosk
    // binding ride `SurfaceSeams::public`, merged beside `build_public_routes`
    // outside the auth stack.
    Seed {
        surface: "org",
        prefix: "/api",
        module: "oxy_api_frontline",
        function: "routes",
    },
    Seed {
        surface: "public",
        prefix: "/api",
        module: "oxy_api_frontline",
        function: "public_routes",
    },
    // `oxy-api-tenancy` (its onboarding module) also fills the `SurfaceSeams::workspace` seam, which lands
    // inside the `/{workspace_id}` tree rather than beside it — two mount
    // points, so two seeds.
    Seed {
        surface: "workspace",
        prefix: "/api/{workspace_id}",
        module: "oxy_api_tenancy::onboarding",
        function: "workspace_routes",
    },
    Seed {
        surface: "workspace",
        prefix: "/api/{workspace_id}",
        module: "oxy_api_source_upload",
        function: "routes",
    },
    Seed {
        surface: "external",
        prefix: "/external/api/{workspace_id}",
        module: "workspace",
        function: "build_external_workspace_routes",
    },
];

/// Trees scanned for handler doc comments. Wider than [`SOURCE_DIRS`] because a
/// handler often lives nowhere near the file that mounts it (the Slack
/// webhooks, the worktree registry).
///
/// REBUILD COST, deliberate — and it cuts against those crates' stated purpose.
/// Both lists watch `crates/api-github/src`, `crates/api-tenancy/src`,
/// `crates/api-documents/src`, `crates/api-frontline/src` and
/// `crates/api-source-upload/src`, none of which `oxy-app` depends on
/// (`oxy-server` mounts them as siblings; they depend on `oxy-app`, not the
/// reverse). Editing any of them therefore re-runs this build script and
/// recompiles `oxy-app` — the workspace's largest crate — before the small
/// crate you actually edited.
///
/// Decoupling their dev loop was the payoff of extracting them (#2978, #2996,
/// and the documents and frontline splits),
/// and this watch reverses it for anyone working inside them — each of them
/// now carries a comment above its `description` saying so and pointing
/// back here. It is the price of listing their routes at all: not watching them
/// means `oxyc routes` omits or stale-lists those surfaces, the exact
/// failure this catalog exists to prevent. A real cost, though, not a free one.
/// The durable fix is to generate the catalog from `oxy-server`, which already
/// depends on every surface crate and is thin; see internal-docs/oxy-api-cli.md.
///
/// The other entries are free: `oxy-app` already depends on those crates.
const DOC_DIRS: &[&str] = &[
    "crates/app/src",
    "crates/agentic/http/src",
    "crates/cameras/src",
    "crates/airhouse/src",
    "crates/oltp/src",
    "crates/api-github/src",
    "crates/api-tenancy/src",
    "crates/api-documents/src",
    "crates/api-frontline/src",
    "crates/api-source-upload/src",
    // `crates/oltp/src/api`, not the whole crate: every handler the OLTP routes
    // name (`handlers::get_connection`, `erd::get_erd`) lives under `api/`, so
    // the wider path would only pull ~15 unrelated modules (`provisioner`,
    // `resolver`, `migration`, …) into the `FnDoc` pool. Free to watch —
    // `oxy-app` already depends on this crate, so editing it recompiles
    // `oxy-app` regardless and the marginal cost is one build-script re-run.
    "crates/oltp/src/api",
];

/// Trees walked for `.route(...)` / `.nest(...)` / `.merge(...)` mounts.
///
/// Narrower than [`DOC_DIRS`] on purpose: widening it would pull probe routers
/// and the worker health-port router into the same name-resolution pool as the
/// real surface. `route_catalog`'s `every_route_tree_is_scanned` test fails if
/// a tree declaring routes is missing from here.
pub const SOURCE_DIRS: &[&str] = &[
    "crates/app/src/server/router",
    "crates/app/src/server/api",
    "crates/app/src/server/feature_flags",
    "crates/agentic/http/src",
    "crates/cameras/src/routes",
    "crates/airhouse/src/api",
    "crates/api-github/src",
    "crates/api-tenancy/src",
    "crates/api-documents/src",
    "crates/api-frontline/src",
    "crates/api-source-upload/src",
    // Mounts `/oltp/me/connection` and `/oltp/me/erd`, merged into the served
    // app as `oxy_oltp::api::router` — `role_manifest.rs` asserts that merge.
    // Missing here since #2851, so both routes were live and absent from the
    // catalog, which is exactly what this list exists to prevent. Free to
    // watch: `oxy-app` already depends on `oxy-oltp`, so it costs no rebuild
    // the way the sibling API crates do.
    "crates/oltp/src/api",
];

/// Ceiling on how deep an inline `Router::new()` chain may nest before the
/// walk gives up. Function-level recursion is bounded by [`Walker::visited`],
/// which is the real cycle guard; this only covers expression nesting.
const MAX_DEPTH: usize = 24;

/// Beats any `module_score`, so a definition in the mounting file itself wins
/// over one a glob import pulled in — matching Rust's own shadowing rule.
const LOCAL_DEFINITION_SCORE: usize = usize::MAX;

/// Cap on a harvested route note. Long enough for the rationale comments the
/// router modules carry, short enough that 600+ of them stay a rounding error
/// in the binary.
const MAX_DESCRIPTION_CHARS: usize = 500;

const METHODS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "head", "options", "trace", "any",
];

/// Walk the router sources and return every endpoint, sorted by path.
///
/// `repo_root` is the workspace root (the parent of `crates/`). Emits the
/// `cargo:rerun-if-changed` lines for everything it reads.
pub fn collect(repo_root: &Path) -> Vec<Route> {
    let idents = crate_idents(repo_root);
    let mut index: Vec<RouterFn> = Vec::new();
    let mut fn_docs: Vec<FnDoc> = Vec::new();
    for dir in DOC_DIRS {
        let dir = repo_root.join(dir);
        println!("cargo:rerun-if-changed={}", dir.display());
        collect_dir(&dir, repo_root, &idents, "", &mut Vec::new(), &mut fn_docs);
    }
    for source_dir in SOURCE_DIRS {
        let dir = repo_root.join(source_dir);
        // Emitted here as well as in the DOC_DIRS loop. Every SOURCE_DIRS entry
        // happens to sit under a DOC_DIRS entry today, so this is redundant —
        // but a duplicate watch line costs nothing, and relying on that
        // containment would let the catalog go quietly stale the day it breaks.
        println!("cargo:rerun-if-changed={}", dir.display());
        collect_dir(
            &dir,
            repo_root,
            &idents,
            source_dir,
            &mut index,
            &mut Vec::new(),
        );
    }

    // No builder anywhere under `repo_root` means the router sources are not
    // where this script looked — in practice, a workspace root that resolved
    // wrong (a vendored build, a path dependency from another workspace). Say
    // so once and emit an empty catalog rather than letting every seed warn:
    // seven lines do not diagnose it better than one, and `oxyc routes`
    // printing `ROUTES — 0 endpoints` behind a clean build log is exactly the
    // silently-wrong listing this whole design exists to prevent.
    //
    // A dependency-only skeleton (`cargo chef cook`) would land here too, if
    // the build script runs there at all — unconfirmed, and not the case worth
    // wording the message around. Gate on the index rather than on the
    // directories existing either way: several SOURCE_DIRS entries ARE crate
    // `src/` roots, so anything that recreates a member's `src/` defeats a
    // directory check.
    //
    // The `rerun-if-changed` lines are already emitted above, so the real
    // sources landing forces a re-run regardless of this branch.
    if index.is_empty() {
        println!(
            "cargo:warning=route catalog: no router builders found under {} — \
             emitting an empty catalog, so `oxyc routes` will list nothing. \
             Check that the workspace root resolved correctly.",
            repo_root.display()
        );
        return Vec::new();
    }

    let mut walker = Walker {
        index,
        fn_docs,
        visited: HashSet::new(),
        out: Vec::new(),
        depth: 0,
    };
    for seed in SEEDS {
        let Some(idx) = walker.lookup_by_module(seed.module, seed.function, None) else {
            // A renamed or removed seed is a real breakage, but failing the
            // build of the whole crate over a help listing is worse than
            // shipping a catalog the completeness test will reject.
            println!(
                "cargo:warning=route catalog: seed {}::{} not found — `oxyc routes` will be incomplete",
                seed.module, seed.function
            );
            continue;
        };
        walker.walk(idx, seed.prefix, seed.surface);
    }

    let mut routes = walker.out;
    // Surface order follows SURFACES (public first, external last) so the
    // generated array, the `--help` listing and `--routes --json` all agree.
    let rank = |s: &str| {
        SURFACES
            .iter()
            .position(|(id, _, _)| *id == s)
            .unwrap_or(usize::MAX)
    };
    routes.sort_by(|a, b| {
        (rank(a.surface), &a.path, &a.method).cmp(&(rank(b.surface), &b.path, &b.method))
    });
    routes.dedup_by(|a, b| {
        if a.path != b.path || a.method != b.method || a.surface != b.surface {
            return false;
        }
        // The same mount reached twice (an inline chain also followed as a
        // delegation): keep whichever copy carries the prose.
        if b.description.is_empty() {
            b.description = std::mem::take(&mut a.description);
        }
        if b.note.is_empty() {
            b.note = std::mem::take(&mut a.note);
        }
        true
    });
    routes
}

/// Recurse a source tree. Either sink may be a throwaway `Vec`, which is how
/// the doc pass and the router pass share one walker.
fn collect_dir(
    dir: &Path,
    repo_root: &Path,
    idents: &HashMap<PathBuf, String>,
    source_dir: &'static str,
    out: &mut Vec<RouterFn>,
    docs: &mut Vec<FnDoc>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    // Sorted: `read_dir` order is filesystem-dependent, and it would otherwise
    // decide which of two equally-scored definitions supplies a description —
    // making the generated catalog differ between machines.
    let mut entries: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            // `tests/` holds probe routers that are not part of the surface.
            if path.file_name().is_some_and(|n| n == "tests") {
                continue;
            }
            collect_dir(&path, repo_root, idents, source_dir, out, docs);
        } else if path.extension().is_some_and(|e| e == "rs") {
            collect_file(&path, repo_root, idents, source_dir, out, docs);
        }
    }
}

fn collect_file(
    path: &Path,
    repo_root: &Path,
    idents: &HashMap<PathBuf, String>,
    source_dir: &'static str,
    out: &mut Vec<RouterFn>,
    docs: &mut Vec<FnDoc>,
) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    // `#[cfg(test)]` modules build probe routers with paths that never ship.
    let commented = truncate_at_test_module(&raw);
    let src = strip_comments(&commented);
    let module = module_segments(path, repo_root, idents);

    // Every documented fn in the file is a candidate handler, whether or not
    // this file also mounts routes.
    for (name, doc) in harvest_fn_docs(&commented) {
        docs.push(FnDoc {
            module: module.clone(),
            name,
            doc,
            file: path.to_path_buf(),
        });
    }

    if !src.contains(".route(")
        && !src.contains(".route_fleet(")
        && !src.contains(".route_ide(")
        && !src.contains(".nest(")
        && !src.contains(".merge(")
    {
        return;
    }
    let (imports, glob_imports) = parse_imports(&src);
    let dir = path.parent().unwrap_or(repo_root).to_path_buf();
    // `strip_comments` preserves offsets, so a span found in `src` indexes the
    // original text too — that is how the comments come back.
    for (name, span) in router_fns(&src) {
        let body = src[span.clone()].to_string();
        let docs = harvest_docs(&body, commented.get(span).unwrap_or(""));
        out.push(RouterFn {
            name,
            module: module.clone(),
            body,
            imports: imports.clone(),
            glob_imports: glob_imports.clone(),
            dir: dir.clone(),
            file: path.to_path_buf(),
            source_dir,
            docs,
        });
    }
}

/// `(path literal, METHOD) -> its documenting comment`, for every `.route(..)`
/// in one builder body.
///
/// `stripped` and `commented` are the same span of the same file — the first
/// with comments blanked, the second untouched — so an offset found by
/// scanning `stripped` (where a commented-out mount cannot appear) reads back
/// the real comment from `commented`.
///
/// Two shapes are picked up: the comment block immediately above the mount,
/// and one sitting between `.route(` and the path literal, which is where
/// rustfmt parks it when the call wraps. Comments above a `.nest(..)` are
/// **not** harvested — a nest covers a whole subtree, and attributing its
/// rationale to every endpoint underneath would be worse than saying nothing.
fn harvest_docs(stripped: &str, commented: &str) -> HashMap<(String, String), Option<String>> {
    let mut out: HashMap<(String, String), Option<String>> = HashMap::new();
    if stripped.len() != commented.len() {
        // Offsets are not aligned (a truncated read, say). Better no notes
        // than notes attached to the wrong route.
        return out;
    }
    let mut i = 0;
    while let Some(rel) = stripped[i..].find(".route(") {
        let at = i + rel;
        let open = at + ".route(".len() - 1;
        i = at + ".route(".len();
        let Some(close) = match_delims(stripped, open, '(', ')') else {
            continue;
        };
        let args = &stripped[open + 1..close];
        let parts = split_top_level(args, ',');
        let Some(literal) = parts.first().and_then(|p| string_literal(p)) else {
            continue;
        };
        // Where the path literal actually sits, so an inline comment before it
        // is in range.
        let literal_at = stripped[open..close]
            .find('"')
            .map(|p| open + p)
            .unwrap_or(open);
        let Some(doc) =
            comment_above(commented, at).or_else(|| comment_above(commented, literal_at))
        else {
            continue;
        };
        for (method, _) in method_calls(&parts[1..].join(",")) {
            match out.entry((literal.clone(), method)) {
                std::collections::hash_map::Entry::Vacant(slot) => {
                    slot.insert(Some(doc.clone()));
                }
                // Two mounts of the same literal and method inside one builder
                // (an inline `.nest`ed router repeating a path). Nothing here
                // can tell them apart, so neither gets a note.
                std::collections::hash_map::Entry::Occupied(mut slot) => {
                    slot.insert(None);
                }
            }
        }
    }
    out
}

/// `(function name, doc comment)` for every `///`-documented item in a file.
///
/// Scans forward rather than backwards from each `fn`, because an attribute
/// block sits between the two — `#[utoipa::path(..)]` and `#[instrument]` are
/// everywhere in this codebase — and skipping attributes forward is a bracket
/// match, while walking back over a multi-line one is not.
fn harvest_fn_docs(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while let Some(rel) = src[i..].find("///") {
        let at = i + rel;
        let line_start = src[..at].rfind('\n').map(|p| p + 1).unwrap_or(0);
        if !src[line_start..at].trim().is_empty() {
            // `///` inside code or a string, not a doc comment.
            i = at + 3;
            continue;
        }
        // Consume the run of doc lines.
        let mut lines: Vec<&str> = Vec::new();
        let mut cursor = line_start;
        while cursor < src.len() {
            let line_end = src[cursor..]
                .find('\n')
                .map(|p| cursor + p)
                .unwrap_or(src.len());
            let Some(text) = src[cursor..line_end].trim().strip_prefix("///") else {
                break;
            };
            lines.push(text.trim());
            cursor = (line_end + 1).min(src.len());
        }
        i = cursor;

        // Skip any attributes between the doc and the item.
        let mut j = cursor;
        loop {
            j += src[j..].len() - src[j..].trim_start().len();
            if bytes.get(j) == Some(&b'#') {
                let Some(open) = src[j..].find('[').map(|p| j + p) else {
                    break;
                };
                let Some(close) = match_delims(src, open, '[', ']') else {
                    break;
                };
                j = close + 1;
                continue;
            }
            break;
        }

        if let Some(name) = fn_name_at(&src[j..]) {
            let doc = flatten_doc(&lines);
            if !doc.is_empty() {
                out.push((name, doc));
            }
        }
    }
    out
}

/// The name of the function `src` starts with, past any
/// `pub` / `pub(crate)` / `async` / `const` / `unsafe` qualifier. `None` when
/// the documented item is not a function.
fn fn_name_at(src: &str) -> Option<String> {
    let mut rest = src.trim_start();
    'qualifiers: loop {
        if let Some(r) = rest.strip_prefix("pub") {
            // `pub(crate)` / `pub(super)` / `pub(in path)`.
            let r = match r.starts_with('(') {
                true => &r[match_delims(r, 0, '(', ')')? + 1..],
                false => r,
            };
            if !r.starts_with(char::is_whitespace) {
                return None;
            }
            rest = r.trim_start();
            continue 'qualifiers;
        }
        for kw in ["async", "unsafe", "const", "extern"] {
            if let Some(r) = rest.strip_prefix(kw)
                && r.starts_with(char::is_whitespace)
            {
                rest = r.trim_start();
                continue 'qualifiers;
            }
        }
        break;
    }
    let after = rest.strip_prefix("fn")?;
    if !after.starts_with(char::is_whitespace) {
        return None;
    }
    let after = after.trim_start();
    let end = after.find(|c: char| !is_ident_char(c))?;
    (end > 0).then(|| after[..end].to_string())
}

/// Join doc lines into one line, dropping the rustdoc furniture an API caller
/// has no use for (code fences, `# Errors`-style headings) and capping length.
fn flatten_doc(lines: &[&str]) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in lines {
        if line.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || line.starts_with('#') {
            continue;
        }
        if !line.is_empty() {
            kept.push(line);
        }
    }
    let joined = kept.join(" ");
    let joined = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        return String::new();
    }
    truncate_chars(&joined, MAX_DESCRIPTION_CHARS)
}

/// The `//` comment block ending just before `at`, flattened to one line.
///
/// Walks back over whitespace, then upward across contiguous comment lines.
/// Returns `None` when the preceding line is code — a mount with no comment of
/// its own must not inherit its neighbour's.
fn comment_above(src: &str, at: usize) -> Option<String> {
    let mut end = src[..at].trim_end().len();
    let mut lines: Vec<&str> = Vec::new();
    while end > 0 {
        let line_start = src[..end].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let line = src[line_start..end].trim();
        let Some(text) = line.strip_prefix("//") else {
            break;
        };
        lines.push(text.trim_start_matches('/').trim());
        if line_start == 0 {
            break;
        }
        end = line_start - 1;
    }
    lines.reverse();
    let joined = lines
        .iter()
        .filter(|l| !l.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let joined = joined.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        return None;
    }
    // Router comments run to paragraphs (the airway-config mount is 30 lines of
    // rationale). Keep enough to be useful, not enough to bloat every binary.
    Some(truncate_chars(&joined, MAX_DESCRIPTION_CHARS))
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    // Break on a word boundary so the tail is not a half word.
    match cut.rfind(' ') {
        Some(p) if p > max / 2 => format!("{}…", &cut[..p]),
        _ => format!("{cut}…"),
    }
}

/// Module path for a source file: the crate ident followed by the directory
/// segments below `src/`. `mod.rs` / `lib.rs` collapse into their directory,
/// so `crates/app/src/server/api/admin/mod.rs` becomes
/// `["oxy_app", "server", "api", "admin"]`.
fn module_segments(
    path: &Path,
    repo_root: &Path,
    idents: &HashMap<PathBuf, String>,
) -> Vec<String> {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    let parts: Vec<&str> = rel
        .iter()
        .filter_map(|s| s.to_str())
        .filter(|s| *s != "crates")
        .collect();
    let mut segs: Vec<String> = Vec::new();
    let mut after_src = false;
    for (i, part) in parts.iter().enumerate() {
        if *part == "src" {
            after_src = true;
            continue;
        }
        if !after_src {
            // Crate directory. `crates/app` builds `oxy-app`, `crates/agentic/http`
            // builds `agentic-http` — the on-disk layout does not carry the
            // package name, so reconstruct it from the path segments.
            continue;
        }
        let stem = part.trim_end_matches(".rs");
        let is_last = i + 1 == parts.len();
        if is_last && (stem == "mod" || stem == "lib") {
            continue;
        }
        segs.push(stem.replace('-', "_"));
    }
    let crate_dir: PathBuf = parts.iter().take_while(|p| **p != "src").collect();
    let ident = idents
        .get(&crate_dir)
        .cloned()
        // A path outside `crates/` (or a manifest we could not read): fall back
        // to the directory name, which is right for most crates anyway.
        .unwrap_or_else(|| crate_dir.to_string_lossy().replace(['/', '-'], "_"));
    let mut out = vec![ident];
    out.append(&mut segs);
    out
}

/// `crates/<dir…> -> package ident`, read from each crate's `Cargo.toml`.
///
/// The on-disk layout does not carry the package name (`crates/app` builds
/// `oxy-app`, `crates/agentic/http` builds `agentic-http`), and a hand-written
/// mapping is one more list to forget when a crate is added — which is exactly
/// how a new API crate goes missing from the catalog without a word.
fn crate_idents(repo_root: &Path) -> HashMap<PathBuf, String> {
    let mut out = HashMap::new();
    let crates = repo_root.join("crates");
    let Ok(entries) = std::fs::read_dir(&crates) else {
        return out;
    };
    for entry in entries.flatten().filter(|e| e.path().is_dir()) {
        // Crates sit at `crates/<name>` or one level deeper
        // (`crates/agentic/http`, `crates/infrastructure/llm/openai`).
        read_crate_ident(&entry.path(), &crates, &mut out, 3);
    }
    out
}

fn read_crate_ident(dir: &Path, crates: &Path, out: &mut HashMap<PathBuf, String>, depth: u32) {
    if depth == 0 {
        return;
    }
    if let Some(name) = package_name(&dir.join("Cargo.toml"))
        && let Ok(rel) = dir.strip_prefix(crates)
    {
        out.insert(rel.to_path_buf(), name.replace('-', "_"));
    }
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten().filter(|e| e.path().is_dir()) {
            read_crate_ident(&entry.path(), crates, out, depth - 1);
        }
    }
}

/// `name = "…"` from a Cargo manifest's `[package]` table. Deliberately a line
/// scan rather than a TOML dependency: a build script for the workspace's
/// largest crate should not pull one in to read one field.
fn package_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        if let Some(value) = line.strip_prefix("name") {
            let value = value.trim_start().strip_prefix('=')?.trim();
            return Some(value.trim_matches('"').to_string());
        }
    }
    None
}

/// Every `fn NAME(..) -> ..Router..` in `src`, paired with the byte range of
/// its body (braces included). Signatures may wrap across lines and carry
/// generics/where clauses, so the return type is matched anywhere between the
/// name and the opening brace.
///
/// A range rather than the text itself, because the caller needs to slice the
/// same span out of the un-stripped source to recover comments.
fn router_fns(src: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let mut out = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while let Some(rel) = src[i..].find("fn ") {
        let start = i + rel;
        // Must be a real token boundary, not the tail of e.g. `async fn`.
        if start > 0 && is_ident_char(bytes[start - 1] as char) {
            i = start + 3;
            continue;
        }
        let after = start + 3;
        let name_end = after
            + src[after..]
                .find(|c: char| !is_ident_char(c))
                .unwrap_or(src.len() - after);
        let name = src[after..name_end].to_string();
        let Some(brace_rel) = src[name_end..].find('{') else {
            break;
        };
        let brace = name_end + brace_rel;
        let signature = &src[name_end..brace];
        if !signature.contains("Router") {
            i = name_end;
            continue;
        }
        let Some(end) = match_delims(src, brace, '{', '}') else {
            i = name_end;
            continue;
        };
        out.push((name, brace..end + 1));
        i = end;
    }
    out
}

/// `(ident -> full path segments, glob-imported module paths)` for every `use`
/// in the file, so a call such as `agentic_router(..)` (imported as
/// `router as agentic_router`) resolves back to `agentic_http::router`, and an
/// unqualified name can still be traced through a `use foo::*`.
fn parse_imports(src: &str) -> (HashMap<String, Vec<String>>, Vec<GlobImport>) {
    let mut out = HashMap::new();
    let mut globs = Vec::new();
    let mut i = 0;
    while let Some(rel) = src[i..].find("use ") {
        let start = i + rel;
        if start > 0 && is_ident_char(src.as_bytes()[start - 1] as char) {
            i = start + 4;
            continue;
        }
        let Some(end_rel) = src[start..].find(';') else {
            break;
        };
        let stmt = &src[start + 4..start + end_rel];
        expand_use(stmt, &mut Vec::new(), &mut out, &mut globs);
        i = start + end_rel;
    }
    (out, globs)
}

/// Flatten one `use` statement, recursing through `{a, b as c, d::{e}}` groups.
fn expand_use(
    stmt: &str,
    prefix: &mut Vec<String>,
    out: &mut HashMap<String, Vec<String>>,
    globs: &mut Vec<GlobImport>,
) {
    let stmt = stmt.trim();
    if stmt.is_empty() {
        return;
    }
    if let Some(brace) = stmt.find('{') {
        let head = stmt[..brace].trim().trim_end_matches("::");
        let base_len = prefix.len();
        prefix.extend(split_path(head));
        let Some(close) = match_delims(stmt, brace, '{', '}') else {
            prefix.truncate(base_len);
            return;
        };
        for item in split_top_level(&stmt[brace + 1..close], ',') {
            expand_use(&item, prefix, out, globs);
        }
        prefix.truncate(base_len);
        return;
    }
    let (path, alias) = match stmt.split_once(" as ") {
        Some((p, a)) => (p.trim(), Some(a.trim())),
        None => (stmt, None),
    };
    let mut segs = prefix.clone();
    segs.extend(split_path(path));
    let Some(last) = segs.last().cloned() else {
        return;
    };
    if last == "self" {
        return;
    }
    if last == "*" {
        segs.pop();
        // Only `crate::…` is unambiguously absolute; see `GlobImport::relative`
        // for why a bare path takes the relative (tighter) reading.
        let relative = segs.first().is_none_or(|head| head != "crate");
        // Strip the prefix for the same reason `handler_doc` does: indexed
        // module paths never contain `super`/`crate`/`self`, so a
        // `use super::dto::*` would otherwise be recorded in a shape that can
        // never match anything.
        segs.retain(|seg| seg != "crate" && seg != "self" && seg != "super");
        if !segs.is_empty() {
            globs.push(GlobImport {
                path: segs,
                relative,
            });
        }
        return;
    }
    out.insert(alias.unwrap_or(&last).to_string(), segs);
}

fn split_path(path: &str) -> Vec<String> {
    path.split("::")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

struct Walker {
    index: Vec<RouterFn>,
    fn_docs: Vec<FnDoc>,
    /// `(builder, prefix, surface)` already walked. A `merge` cycle would
    /// otherwise recurse forever, and the same builder legitimately mounts at
    /// two prefixes (`automation_router` serves both `/agentic-workflows` and
    /// `/agentic-automations`) — so the prefix is part of the key, and only a
    /// genuinely redundant re-walk is skipped.
    visited: HashSet<(usize, String, &'static str)>,
    out: Vec<Route>,
    depth: usize,
}

impl Walker {
    /// Walk one builder function, mounting everything it declares under `prefix`.
    fn walk(&mut self, fn_idx: usize, prefix: &str, surface: &'static str) {
        if !self.visited.insert((fn_idx, prefix.to_string(), surface)) {
            return;
        }
        let body = self.index[fn_idx].body.clone();
        let lets = parse_let_bindings(&body);
        self.depth += 1;
        self.descend(&body, prefix, surface, fn_idx, &lets);
        self.depth -= 1;
    }

    /// Follow a router-valued expression, mounting whatever it builds under
    /// `prefix`. Handles the four shapes the router modules actually use: an
    /// inline `Router::new()` chain, a bare identifier bound by an earlier
    /// `let`, a call to another builder, and a builder that merely delegates
    /// (`operator::routes::<S>().layer(..)`).
    ///
    /// `caller` supplies the import scope for resolving builder names.
    fn descend(
        &mut self,
        expr: &str,
        prefix: &str,
        surface: &'static str,
        caller: usize,
        lets: &HashMap<String, String>,
    ) {
        if self.depth > MAX_DEPTH {
            return;
        }
        // Unwrap `{ … }` (a function body, or a block passed to `.merge(..)`)
        // so statement boundaries inside sit at nesting depth zero.
        let trimmed = expr.trim();
        if trimmed.starts_with('{')
            && let Some(close) = match_delims(trimmed, 0, '{', '}')
            && close == trimmed.len() - 1
        {
            let inner = trimmed[1..close].to_string();
            self.depth += 1;
            self.descend(&inner, prefix, surface, caller, lets);
            self.depth -= 1;
            return;
        }
        let bare = expr.trim().trim_end_matches(',').trim();
        if !bare.is_empty() && bare.chars().all(is_ident_char) {
            if let Some(body) = lets.get(bare) {
                let body = body.clone();
                self.depth += 1;
                self.descend(&body, prefix, surface, caller, lets);
                self.depth -= 1;
            }
            return;
        }

        if next_marker(expr, 0).is_none() {
            // No mounts of its own: this expression just hands back another
            // builder's router, so follow every call it makes. Calls that do
            // not resolve to an indexed builder (`middleware::from_fn`, …)
            // are simply ignored.
            for call in call_paths(expr) {
                if let Some(idx) = self.resolve(&call, caller) {
                    self.walk(idx, prefix, surface);
                }
            }
            return;
        }

        self.depth += 1;
        let mut i = 0;
        let mut seen_head: Option<usize> = None;
        while i < expr.len() {
            let Some((marker, at)) = next_marker(expr, i) else {
                break;
            };
            // The chain this marker hangs off may START with another builder
            // (`feature_flags::routes::router().merge(..)`), which mounts at
            // the same prefix. Follow it once per chain.
            let head = chain_start(expr, at);
            if seen_head != Some(head) {
                seen_head = Some(head);
                for call in call_paths(&expr[head..at]) {
                    if let Some(idx) = self.resolve(&call, caller) {
                        self.walk(idx, prefix, surface);
                    }
                }
            }
            let open = at + marker.len() - 1;
            let Some(close) = match_delims(expr, open, '(', ')') else {
                i = at + marker.len();
                continue;
            };
            let args = &expr[open + 1..close];
            match marker {
                // `route_split` puts the path first too, then alternating
                // method/handler pairs; `method_calls` picks the handlers out of
                // the tail either way.
                ".route(" | ".route_fleet(" | ".route_ide(" | ".route_split(" => {
                    self.emit_route(args, prefix, surface, caller)
                }
                ".nest(" | ".nest_service(" | ".nest_declared(" | ".nest_typed(" | ".nest_all(" => {
                    let parts = split_top_level(args, ',');
                    // `nest_all` and `nest_typed` put the ROLE second and the
                    // router third — `(prefix, role, router, …)`. Descending into
                    // `RouteRole::FleetOk` finds nothing, which is how the whole
                    // `/api/admin` subtree went missing from the catalog without
                    // anything failing to compile.
                    let inner_at = match marker {
                        ".nest_all(" | ".nest_typed(" => 2,
                        _ => 1,
                    };
                    if let (Some(seg), Some(inner)) = (parts.first(), parts.get(inner_at))
                        && let Some(seg) = string_literal(seg)
                    {
                        self.descend(inner, &join(prefix, &seg), surface, caller, lets);
                    }
                }
                // The declared/undeclared variants carry a second argument
                // (declarations, or the reason a merge has none). `descend`
                // scans text for markers, so the extra argument is inert.
                ".merge(" | ".merge_declared(" | ".merge_undeclared(" => {
                    self.descend(args, prefix, surface, caller, lets)
                }
                _ => {}
            }
            i = close + 1;
        }
        self.depth -= 1;
    }

    fn emit_route(&mut self, args: &str, prefix: &str, surface: &'static str, caller: usize) {
        let parts = split_top_level(args, ',');
        let Some(path) = parts.first().and_then(|p| string_literal(p)) else {
            return;
        };
        let rest = parts[1..].join(",");
        let full = join(prefix, &path);
        for (method, handler) in method_calls(&rest) {
            let note = self.index[caller]
                .docs
                .get(&(path.clone(), method.clone()))
                .cloned()
                .flatten()
                .unwrap_or_default();
            let description = self.handler_doc(&handler, caller);
            self.out.push(Route {
                method,
                path: full.clone(),
                handler,
                surface,
                source_dir: self.index[caller].source_dir,
                description,
                note,
            });
        }
    }

    /// The doc comment on the handler a route dispatches to, resolved the same
    /// way builder names are: expand the leading segment through the mounting
    /// file's imports, then suffix-match the module path.
    fn handler_doc(&self, handler: &str, caller: usize) -> String {
        if handler.is_empty() {
            return String::new();
        }
        let call = split_path(handler);
        let imports = &self.index[caller].imports;
        let mut segs: Vec<String> = match call.split_first() {
            Some((head, tail)) => match imports.get(head) {
                Some(full) => full.iter().chain(tail).cloned().collect(),
                None => call.clone(),
            },
            None => return String::new(),
        };
        let Some(name) = segs.pop() else {
            return String::new();
        };
        let module: Vec<String> = segs
            .into_iter()
            .filter(|s| s != "crate" && s != "self" && s != "super")
            .collect();

        let mut best: Option<(usize, &str)> = None;
        for d in &self.fn_docs {
            if d.name != name {
                continue;
            }
            let score = if module.is_empty() {
                // An unqualified name is either defined in the mounting file
                // itself, or pulled in by a glob (`pub use handlers::*`) — a
                // named import would already have been expanded above. Anything
                // else with this name is a coincidence and must not match.
                if d.file == self.index[caller].file {
                    // A local item shadows a glob import, as in Rust itself, so
                    // it has to outscore every glob match rather than tie.
                    LOCAL_DEFINITION_SCORE
                } else {
                    // A *relative* glob resolves against the caller's own module
                    // tree, so a definition outside it cannot be what the glob
                    // brought in — without that gate, `use handlers::*`
                    // suffix-matches every module named `handlers` in the
                    // workspace (nine of them). An absolute `crate::…::*` names
                    // a module anywhere and is left alone.
                    //
                    // The relative gate is the file's parent directory, which
                    // for a non-`mod.rs` file admits its whole sibling tree
                    // rather than just its own submodules — loose, in the safe
                    // direction.
                    //
                    // An absolute `crate::…::*` is confined to the caller's own
                    // crate, which is what `crate::` means. Skipping the check
                    // entirely would just move the over-matching: `module_score`
                    // is a *suffix* match, so an unconfined absolute glob could
                    // take a description from a same-named module in a
                    // different crate.
                    let caller = &self.index[caller];
                    let same_crate = d.module.first() == caller.module.first();
                    match caller
                        .glob_imports
                        .iter()
                        .filter(|glob| {
                            if glob.relative {
                                d.file.starts_with(&caller.dir)
                            } else {
                                same_crate
                            }
                        })
                        .filter_map(|glob| module_score(&d.module, &glob.path))
                        .max()
                    {
                        Some(score) => score,
                        None => continue,
                    }
                }
            } else {
                match module_score(&d.module, &module) {
                    Some(score) => score,
                    None => continue,
                }
            };
            if best.is_none_or(|(b, _)| score > b) {
                best = Some((score, d.doc.as_str()));
            }
        }
        best.map(|(_, doc)| doc.to_string()).unwrap_or_default()
    }

    /// Resolve a call path to an indexed builder, preferring the definition
    /// nearest the caller when a `(module, fn)` pair is ambiguous.
    fn resolve(&self, call: &[String], caller: usize) -> Option<usize> {
        let imports = &self.index[caller].imports;
        let mut segs: Vec<String> = match call.split_first() {
            Some((head, tail)) => match imports.get(head) {
                Some(full) => full.iter().chain(tail).cloned().collect(),
                None => call.to_vec(),
            },
            None => return None,
        };
        let name = segs.pop()?;
        let module: Vec<String> = segs
            .into_iter()
            .filter(|s| s != "crate" && s != "self" && s != "super")
            .collect();
        self.lookup(&module, &name, Some(caller))
    }

    fn lookup_by_module(&self, module: &str, name: &str, caller: Option<usize>) -> Option<usize> {
        self.lookup(&split_path(module), name, caller)
    }

    /// Pick the indexed builder whose module path ends with `module` and which
    /// defines `name`. Longest suffix match wins; ties break toward the
    /// caller's own directory, which is what disambiguates the several
    /// `router()` builders that share a leaf module name.
    fn lookup(&self, module: &[String], name: &str, caller: Option<usize>) -> Option<usize> {
        let mut best: Option<(usize, usize, bool)> = None;
        for (i, f) in self.index.iter().enumerate() {
            if f.name != name {
                continue;
            }
            let score = suffix_len(&f.module, module);
            if module.is_empty() {
                // Unqualified call: only the caller's own file can define it.
                if caller.is_some_and(|c| self.index[c].dir != f.dir) {
                    continue;
                }
            } else if score == 0 {
                continue;
            }
            let same_dir = caller.is_some_and(|c| self.index[c].dir == f.dir);
            let better = match best {
                None => true,
                Some((_, bs, bd)) => score > bs || (score == bs && same_dir && !bd),
            };
            if better {
                best = Some((i, score, same_dir));
            }
        }
        best.map(|(i, _, _)| i)
    }
}

/// How well a candidate module matches the module a handler path names.
///
/// An exact suffix match scores highest. One level below that covers the
/// dominant re-export shape in this codebase — `auth/mod.rs` doing
/// `pub use handlers::*`, so `auth::get_config` is really
/// `auth::handlers::get_config` — without opening the door to matching any
/// same-named function anywhere.
fn module_score(candidate: &[String], query: &[String]) -> Option<usize> {
    if query.is_empty() {
        return None;
    }
    if suffix_len(candidate, query) > 0 {
        return Some(query.len() * 2);
    }
    if candidate.len() > 1 && suffix_len(&candidate[..candidate.len() - 1], query) > 0 {
        return Some(query.len() * 2 - 1);
    }
    None
}

/// Longest common suffix length between a module path and a query path.
fn suffix_len(module: &[String], query: &[String]) -> usize {
    if query.is_empty() || query.len() > module.len() {
        return 0;
    }
    let tail = &module[module.len() - query.len()..];
    if tail == query { query.len() } else { 0 }
}

/// `let NAME = <router expr>;` bindings in a builder body, so a `.merge(x)` of
/// a locally-built router still contributes its routes.
fn parse_let_bindings(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut i = 0;
    while let Some(rel) = body[i..].find("let ") {
        let start = i + rel;
        if start > 0 && is_ident_char(body.as_bytes()[start - 1] as char) {
            i = start + 4;
            continue;
        }
        let after = start + 4;
        let name_start = after + body[after..].len() - body[after..].trim_start().len();
        let rest = &body[name_start..];
        let rest = rest.strip_prefix("mut ").unwrap_or(rest);
        let name_start = body.len() - rest.len();
        let name_end = name_start + rest.find(|c: char| !is_ident_char(c)).unwrap_or(rest.len());
        let name = body[name_start..name_end].to_string();
        let Some(eq) = body[name_end..].find('=') else {
            break;
        };
        let value_start = name_end + eq + 1;
        let Some(semi) = find_top_level_semicolon(&body[value_start..]) else {
            i = value_start;
            continue;
        };
        let value = body[value_start..value_start + semi].to_string();
        if value.to_ascii_lowercase().contains("route") {
            out.insert(name, value);
        }
        i = value_start + semi;
    }
    out
}

fn find_top_level_semicolon(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    for (i, c) in s.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ';' if depth <= 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Start of the builder chain that the marker at `at` hangs off: the position
/// just past the nearest preceding statement/argument boundary at depth zero.
/// `let x = foo::router().merge(..)` yields the offset of `foo`.
fn chain_start(expr: &str, at: usize) -> usize {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0;
    let mut chars = expr[..at].char_indices();
    while let Some((i, c)) = chars.next() {
        if in_str {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            // `=` also catches `=>`, which is the boundary we want in a match arm.
            ';' | '=' | ',' if depth == 0 => start = i + 1,
            _ => {}
        }
    }
    start
}

/// Next `.route(` / `.nest(` / `.nest_service(` / `.merge(` at or after `from`.
fn next_marker(s: &str, from: usize) -> Option<(&'static str, usize)> {
    // The declaring doors count too. `RoleRouter` (crates/app/src/server/router/
    // role_router.rs) replaced bare `.route(` across the workspace and public
    // trees so a route's pod is a type rather than a note — and a catalog that
    // only knows `.route(` sees an empty router and says so with an empty
    // table, which is the silence this whole area exists to remove. `.route(` is
    // not a prefix of `.route_fleet(`: the trailing `(` makes every match exact.
    const MARKERS: &[&str] = &[
        ".route(",
        ".route_fleet(",
        ".route_ide(",
        ".route_split(",
        ".nest(",
        ".nest_service(",
        ".nest_declared(",
        ".nest_typed(",
        ".nest_all(",
        ".merge(",
        ".merge_declared(",
        ".merge_undeclared(",
    ];
    let mut best: Option<(&'static str, usize)> = None;
    for m in MARKERS {
        if let Some(rel) = s[from..].find(m) {
            let at = from + rel;
            // `.nest(` is a prefix of nothing, but `.nest_service(` would be
            // missed if `.nest(` matched first — they cannot overlap because
            // the trailing `(` makes each match exact.
            if best.is_none_or(|(_, b)| at < b) {
                best = Some((m, at));
            }
        }
    }
    best
}

/// `(METHOD, handler)` for every method-router call in a `.route(..)` second
/// argument: `get(a).post(b)` yields GET/`a` and POST/`b`.
fn method_calls(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < s.len() {
        if !is_ident_char(s[i..].chars().next().unwrap_or(' ')) {
            i += 1;
            continue;
        }
        if i > 0 && is_ident_char(bytes[i - 1] as char) {
            i += 1;
            continue;
        }
        let end = i + s[i..]
            .find(|c: char| !is_ident_char(c))
            .unwrap_or(s.len() - i);
        let ident = &s[i..end];
        if METHODS.contains(&ident) && s[end..].starts_with('(') {
            if let Some(close) = match_delims(s, end, '(', ')') {
                let handler = first_arg(&s[end + 1..close]);
                out.push((ident.to_uppercase(), handler));
                i = end + 1;
                continue;
            }
        }
        i = end.max(i + 1);
    }
    out
}

/// Leading path expression of a handler argument, e.g. `workspaces::get_workspace`.
/// Closures and other non-path expressions collapse to an empty string.
fn first_arg(args: &str) -> String {
    let arg = split_top_level(args, ',')
        .into_iter()
        .next()
        .unwrap_or_default();
    let arg = arg.trim();
    let path: String = arg
        .chars()
        .take_while(|c| is_ident_char(*c) || *c == ':')
        .collect();
    if path.is_empty() {
        return String::new();
    }
    path.trim_end_matches(':').to_string()
}

/// Function-call paths in an expression, turbofish stripped: `audit::router()`
/// yields `["audit", "router"]`. Method calls (`x.foo()`) are skipped — only
/// free functions and associated paths can name another router builder.
fn call_paths(expr: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let bytes = expr.as_bytes();
    let mut i = 0;
    while i < expr.len() {
        if !is_ident_char(bytes[i] as char) {
            i += 1;
            continue;
        }
        // Only start at a path boundary; `.foo(` is a method call, and a `::`
        // to the left means we are mid-path and already consumed the head.
        let is_start = i == 0
            || (!is_ident_char(bytes[i - 1] as char)
                && bytes[i - 1] != b'.'
                && !expr[..i].ends_with("::"));
        if !is_start {
            i += 1;
            while i < expr.len() && is_ident_char(bytes[i] as char) {
                i += 1;
            }
            continue;
        }
        // Consume `ident(::ident)*`.
        let start = i;
        let mut end = i;
        loop {
            end += expr[end..]
                .find(|c: char| !is_ident_char(c))
                .unwrap_or(expr.len() - end);
            if expr[end..].starts_with("::") && expr[end + 2..].starts_with(is_ident_char) {
                end += 2;
                continue;
            }
            break;
        }
        // Skip a turbofish so `router::<AppState>(..)` still reads as a call.
        let mut after = end;
        if expr[after..].starts_with("::<")
            && let Some(close) = match_delims(expr, after + 2, '<', '>')
        {
            after = close + 1;
        }
        if expr[after..].trim_start().starts_with('(') {
            let path = split_path(&expr[start..end]);
            let leaf_is_method = path.last().is_some_and(|l| METHODS.contains(&l.as_str()));
            if !path.is_empty() && !leaf_is_method {
                out.push(path);
            }
        }
        i = end.max(i + 1);
    }
    out
}

fn join(prefix: &str, segment: &str) -> String {
    let a = prefix.trim_end_matches('/');
    let b = segment.trim_start_matches('/').trim_end_matches('/');
    if b.is_empty() {
        if a.is_empty() {
            "/".to_string()
        } else {
            a.to_string()
        }
    } else {
        format!("{a}/{b}")
    }
}

/// The string literal an expression starts with, if any.
fn string_literal(expr: &str) -> Option<String> {
    let e = expr.trim();
    let rest = e.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Index of the delimiter closing the one at `open`, respecting nesting and
/// string literals.
fn match_delims(s: &str, open: usize, oc: char, cc: char) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut chars = s[open..].char_indices();
    while let Some((i, c)) = chars.next() {
        if in_str {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            _ if c == oc => depth += 1,
            _ if c == cc => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + i);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split on `sep` at nesting depth zero, ignoring separators inside strings,
/// parens, brackets, braces and generic argument lists.
fn split_top_level(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut start = 0;
    let mut chars = s.char_indices();
    while let Some((i, c)) = chars.next() {
        if in_str {
            if c == '\\' {
                chars.next();
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if c == sep && depth == 0 => {
                out.push(s[start..i].to_string());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(s[start..].to_string());
    out.into_iter().filter(|p| !p.trim().is_empty()).collect()
}

/// Everything before the first top-level `#[cfg(test)]`.
fn truncate_at_test_module(src: &str) -> String {
    match src.find("\n#[cfg(test)]") {
        Some(i) => src[..i].to_string(),
        None => src.to_string(),
    }
}

/// Blank out comments so a commented-out `.route(...)` never reaches the walk,
/// and so a comment inside a builder call cannot hide the path literal that
/// follows it. String literals — including raw strings, which the rest of the
/// walker never sees because they do not appear in builder chains — are
/// preserved verbatim.
///
/// Comments are replaced **byte for byte with spaces** (newlines kept) rather
/// than removed, so the result is offset-identical to the input. That is what
/// lets [`harvest_docs`] read the original comment sitting above a mount that
/// the walker found in the stripped text.
fn strip_comments(src: &str) -> String {
    let bytes = src.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];

        // Raw string: r"…", r#"…"#, br##"…"##. Copied through untouched, so a
        // `//` or a quote inside one cannot desynchronise the scan.
        if (c == b'r' || c == b'b')
            && !(i > 0 && is_ident_char(bytes[i - 1] as char))
            && let Some((start, hashes)) = raw_string_open(bytes, i)
        {
            let end = raw_string_close(bytes, start, hashes);
            out.extend_from_slice(&bytes[i..end]);
            i = end;
            continue;
        }

        if c == b'"' {
            let end = quoted_end(bytes, i);
            out.extend_from_slice(&bytes[i..end]);
            i = end;
            continue;
        }
        // A char literal, which may be `'"'` — copied through so its quote
        // cannot open a phantom string and desynchronise everything after it.
        // Lifetimes (`'a`) look the same at this byte and are not skipped.
        if c == b'\''
            && let Some(end) = char_literal_end(bytes, i)
        {
            out.extend_from_slice(&bytes[i..end]);
            i = end;
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(b' ');
                i += 1;
            }
            continue;
        }
        if c == b'/' && bytes.get(i + 1) == Some(&b'*') {
            let close = find_block_comment_end(bytes, i + 2);
            for b in &bytes[i..close] {
                out.push(if *b == b'\n' { b'\n' } else { b' ' });
            }
            i = close;
            continue;
        }
        out.push(c);
        i += 1;
    }
    // Only whole comment spans were replaced, and only with ASCII, so every
    // surviving byte is still part of its original UTF-8 sequence.
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Index just past the `*/` closing the block comment whose body starts at `i`.
fn find_block_comment_end(bytes: &[u8], mut i: usize) -> usize {
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'/' {
            return i + 2;
        }
        i += 1;
    }
    bytes.len()
}

/// If a raw-string literal opens at `i`, return `(index just past its opening
/// quote, number of `#`)`. Accepts the `b` prefix so `br#"…"#` is recognised.
fn raw_string_open(bytes: &[u8], i: usize) -> Option<(usize, usize)> {
    let mut j = i;
    if bytes[j] == b'b' {
        j += 1;
    }
    if bytes.get(j) != Some(&b'r') {
        return None;
    }
    j += 1;
    let hash_start = j;
    while bytes.get(j) == Some(&b'#') {
        j += 1;
    }
    if bytes.get(j) != Some(&b'"') {
        return None;
    }
    Some((j + 1, j - hash_start))
}

/// Index just past the terminator of a raw string whose body starts at `start`.
fn raw_string_close(bytes: &[u8], start: usize, hashes: usize) -> usize {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'"' && bytes[i + 1..].iter().take(hashes).all(|b| *b == b'#') {
            let end = i + 1 + hashes;
            if end <= bytes.len() {
                return end;
            }
        }
        i += 1;
    }
    bytes.len()
}

/// Index just past the closing `'` of a char literal starting at `i`, or `None`
/// when this apostrophe opens a lifetime instead.
///
/// A char literal is `'x'` or `'\\n'`-style escape; a lifetime is `'` followed
/// by an identifier and no closing quote. Distinguishing them is what keeps
/// `'"'` from being read as the start of a string.
fn char_literal_end(bytes: &[u8], i: usize) -> Option<usize> {
    if bytes.get(i + 1) == Some(&b'\\') {
        // Escape: scan past the escaped byte to the closing quote (`'\n'`,
        // `'\u{1F600}'`). Starting at `i + 3`, not `i + 2`, is what makes
        // `'\''` work — from `i + 2` the scan finds the *escaped* quote and
        // stops one byte short of the real terminator.
        let close = bytes[i + 3..].iter().position(|b| *b == b'\'')?;
        return Some(i + 4 + close);
    }
    // Exactly one character between the quotes. Multi-byte chars are UTF-8, so
    // step over the whole sequence rather than a single byte.
    let width = utf8_width(*bytes.get(i + 1)?);
    (bytes.get(i + 1 + width) == Some(&b'\'')).then_some(i + 2 + width)
}

/// Byte length of the UTF-8 sequence a leading byte starts.
fn utf8_width(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// Index just past the closing quote of the ordinary string literal opening at
/// `i`, honouring backslash escapes.
fn quoted_end(bytes: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < bytes.len() {
        match bytes[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    bytes.len()
}

/// Display order for the surfaces a route can sit on, with the credential each
/// one expects. Every `surface` used in [`SEEDS`] must appear here; the
/// generated table is what `oxyc api` renders, so this is the single source.
pub const SURFACES: &[(&str, &str, &str)] = &[
    (
        "public",
        "PUBLIC",
        "no credential required (health, auth handshake, custom-app runtime)",
    ),
    (
        "org",
        "ACCOUNT & ORG",
        "your user token; `/api/admin/*` additionally needs platform standing",
    ),
    (
        "workspace",
        "WORKSPACE",
        "your user token; `{workspace_id}` must be one you can access",
    ),
    (
        "cameras",
        "DEVICE",
        "camera-fleet edge endpoints, authenticated by a device token, not a user",
    ),
    (
        "external",
        "EXTERNAL (/external/api)",
        "API key only (X-API-Key header), never a browser cookie",
    ),
];
