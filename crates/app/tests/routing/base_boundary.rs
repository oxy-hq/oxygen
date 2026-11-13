//! The base, marked out: which of `oxy-app` goes *down* when the crate
//! dissolves, and every reach from it into what does not.
//!
//! Step (c) of "End state: no `oxy-app`" (`internal-docs/domain-boundaries.md`).
//! `oxy-app` splits three ways: up into `oxy-server`, sideways into contexts,
//! and down into the base every surface imports through the prelude. [`BASE`]
//! names the modules of that third part. Nothing moves here; this test draws the
//! line a later move down follows, and keeps it from blurring meanwhile.
//!
//! The base can only leave `oxy-app` once it imports nothing that stays behind.
//! [`BACKLOG`] is the list of those imports: each row is a base module reaching
//! a module outside the base, and is cleared when one of the two moves. It is a
//! backlog, not an exemption list: a new reach fails, and so does a row that no
//! longer matches anything, so the list can only shrink.
//!
//! Sources are parsed with `syn`, so `use` trees with nested braces, `super::`
//! inside inline modules, and `crate::` paths inside macro calls all resolve.
//! A re-export of a lower crate (`pub use oxy_app_core::compiled_reader`) is
//! followed to that crate, so reaching it is not a reach into `oxy-app`. Test
//! code (`#[cfg(test)]` modules and `*_tests.rs` files) is not scanned: a base
//! module's tests move with it and bring their own fixtures.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use syn::visit::Visit;

/// The modules of `oxy-app` that go down: what the surfaces import through
/// `oxy_app::surface` as platform, and the workspace runtime under it. Module
/// paths below `crate::`; a module covers everything under it.
const BASE: &[&str] = &[
    // Guards and request context: role guards, org and workspace context,
    // the staff-console guards, token scopes, request id, timeout.
    "server::api::middlewares",
    // Session helpers every sign-in path shares. The sign-in handlers beside
    // them (`handlers`, `dev_login`, `anonymous_link`, `mobile`) are the
    // identity context, not base.
    "server::api::auth::ops",
    "server::api::auth::cookie_session",
    "server::api::auth::token_session",
    "server::api::auth::dto",
    // The authz shim over `oxy-server-authz`.
    "server::authz",
    // Pod placement: route roles, the role middleware, the Factory relay,
    // servability.
    "server::role_manifest",
    "server::role_middleware",
    "server::ide_proxy",
    "server::servability",
    "server::router::role_router",
    // Staff scope and assume-role sessions.
    "server::api::admin::scope",
    "server::api::admin::org_filter",
    "server::api::admin::assume",
    // The seam types surfaces fill, and the router's semantic caches.
    "server::router::seams",
    "server::router::workspace_cache",
    // The LISTEN/NOTIFY settings a surface's listener rides.
    "server::task_router_listener",
    // The workspace runtime: `OxyProjectContext` and the agentic wiring, and
    // the workspace manager the global driver builds. Its own crate when it
    // leaves (too big to share a rebuild with the rest).
    "agentic_wiring",
    "server::preagg_workspace",
    // The runtime-artifact S3 mirror (`oxy-compile`'s blob store only).
    "server::runtime_artifact",
];

/// `(base module, target prefix)`: a reach from the base into a module that
/// is not base. Shrink-only. Each clears when its target moves (down, or out
/// to its context's crate) or the base module stops needing it.
const BACKLOG: &[(&str, &str)] = &[
    // The workspace runtime reaches into contexts it serves. Previews: the
    // request-scoped write hold and the preview DDL/registry the preview
    // context builds on. Clears when previews leaves (or its hold goes down).
    ("agentic_wiring", "server::previews"),
    // The compile dispatcher builds the compile worker; the monitor scan
    // queues an anomaly notice; the project context reads the compiled scan
    // source; the metric-tree runner streams through `typed_stream`.
    ("agentic_wiring", "server::compile_worker"),
    ("agentic_wiring", "server::anomaly_notify"),
    ("agentic_wiring", "server::api::semantic"),
    ("agentic_wiring", "server::api::typed_stream"),
    // Session helpers mint through the sign-in handlers: `create_auth_token`
    // lives in `handlers`, the anonymous link in `anonymous_link`. Base items
    // in identity files; they clear when the helpers move beside `ops`.
    (
        "server::api::auth::ops",
        "server::api::auth::anonymous_link",
    ),
    ("server::api::auth::ops", "server::api::auth::handlers"),
    // The workspace middleware: branch self-heal, the reconcile check, the
    // Factory-access gate, preview pins and read-only holds, the worktree
    // registry and the detached-branch status. The largest knot: it is where
    // request context meets every context that can refuse a request.
    ("server::api::middlewares", "server::api::workspaces"),
    // The subscription guard asks billing whether billing is switched off.
    ("server::api::middlewares", "server::api::billing"),
    ("server::api::middlewares", "server::compile_git"),
    ("server::api::middlewares", "server::compile_reconcile"),
    ("server::api::middlewares", "server::factory_access"),
    ("server::api::middlewares", "server::feature_flags"),
    ("server::api::middlewares", "server::previews"),
    // `IdeState` and `FleetState` are base types defined in the router's
    // `mod.rs`, which goes up last. They clear when the two move beside
    // `role_router`.
    ("server::api::middlewares", "server::router::IdeState"),
    ("server::router::role_router", "server::router::IdeState"),
    ("server::router::role_router", "server::router::FleetState"),
    // The test-only installer builds the real routes to read their roles.
    (
        "server::role_manifest",
        "server::router::route_declarations",
    ),
    // `SurfaceSeams::catalog` carries the route catalog type.
    ("server::router::seams", "server::route_catalog"),
];

// ---------------------------------------------------------------------------

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The module path a source file declares, below `crate::`.
fn file_module(src: &Path, file: &Path) -> Vec<String> {
    let rel = file
        .strip_prefix(src)
        .expect("under src")
        .with_extension("");
    let mut segs: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if matches!(segs.last().map(String::as_str), Some("mod" | "lib")) {
        segs.pop();
    }
    segs
}

fn is_test_file(file: &Path) -> bool {
    let stem = file.file_stem().unwrap_or_default().to_string_lossy();
    stem == "tests"
        || stem.ends_with("_tests")
        || stem == "test_support"
        || file.components().any(|c| c.as_os_str() == "tests")
}

fn under(path: &[String], prefix: &[String]) -> bool {
    path.len() >= prefix.len() && path[..prefix.len()] == *prefix
}

fn segs(s: &str) -> Vec<String> {
    s.split("::").map(str::to_string).collect()
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.meta
                .require_list()
                .is_ok_and(|l| l.tokens.to_string().split_whitespace().any(|t| t == "test"))
    })
}

/// A path as written, resolved against the module it appears in: a leading
/// `crate` / `self` / `super`, or a child module of this one (`ops::x` beside
/// `mod ops;`, which the 2018 edition resolves without `self::`). `None` for a
/// path that starts at an extern crate or an item in scope, which cannot name
/// an `oxy-app` module this test has not already followed through a `use`.
fn resolve(
    modules: &BTreeSet<Vec<String>>,
    module: &[String],
    written: &[String],
) -> Option<Vec<String>> {
    let first = written.first()?;
    let mut base: Vec<String> = match first.as_str() {
        "crate" => Vec::new(),
        "self" | "super" => module.to_vec(),
        _ => {
            let mut child = module.to_vec();
            child.push(first.clone());
            if !modules.contains(&child) {
                return None;
            }
            child.pop();
            child.extend(
                written
                    .iter()
                    .filter(|s| *s != "*" && *s != "self")
                    .cloned(),
            );
            return Some(child);
        }
    };
    let mut rest = &written[1..];
    if written[0] == "super" {
        base.pop();
    }
    while rest.first().is_some_and(|s| s == "super") {
        base.pop();
        rest = &rest[1..];
    }
    base.extend(rest.iter().filter(|s| *s != "*" && *s != "self").cloned());
    Some(base)
}

/// One file's view: the paths it reaches, and the re-exports it declares.
#[derive(Default)]
struct Scan {
    /// `(module the path appears in, path as written)`.
    reaches: Vec<(Vec<String>, Vec<String>)>,
    /// `(alias, target as written, module it was declared in)` for every `use`
    /// with a visibility (a re-export).
    reexports: Vec<(Vec<String>, Vec<String>, Vec<String>)>,
    /// `(module, name) -> path as written` for every `use` in the file, so a
    /// later `name::item` resolves through the import.
    locals: BTreeMap<(Vec<String>, String), Vec<String>>,
    /// Every module this file declares (`mod x;` and inline), as full paths.
    mods: Vec<Vec<String>>,
}

struct Visitor<'a> {
    module: Vec<String>,
    scan: &'a mut Scan,
}

fn flatten_use(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, String)>,
) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten_use(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let name = n.ident.to_string();
            let mut full = prefix.clone();
            if name != "self" {
                full.push(name.clone());
            }
            let alias = if name == "self" {
                prefix.last().cloned().unwrap_or_default()
            } else {
                name
            };
            out.push((full, alias));
        }
        syn::UseTree::Rename(r) => {
            let mut full = prefix.clone();
            if r.ident != "self" {
                full.push(r.ident.to_string());
            }
            out.push((full, r.rename.to_string()));
        }
        syn::UseTree::Glob(_) => {
            let mut full = prefix.clone();
            full.push("*".into());
            out.push((full, "*".into()));
        }
        syn::UseTree::Group(g) => {
            for t in &g.items {
                flatten_use(t, prefix, out);
            }
        }
    }
}

/// `crate :: a :: b` inside a macro's tokens, which `syn` leaves unparsed.
fn macro_paths(tokens: &str) -> Vec<Vec<String>> {
    let words: Vec<&str> = tokens.split_whitespace().collect();
    let mut out = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if !matches!(*w, "crate" | "super" | "self") || words.get(i + 1) != Some(&"::") {
            continue;
        }
        if i > 0 && words[i - 1] == "::" {
            continue;
        }
        let mut path = vec![w.to_string()];
        let mut j = i + 1;
        while words.get(j) == Some(&"::") {
            match words.get(j + 1) {
                Some(next) if next.chars().all(|c| c.is_alphanumeric() || c == '_') => {
                    path.push(next.to_string());
                    j += 2;
                }
                _ => break,
            }
        }
        out.push(path);
    }
    out
}

impl<'ast> Visit<'ast> for Visitor<'_> {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        if is_cfg_test(&m.attrs) {
            return;
        }
        let mut declared = self.module.clone();
        declared.push(m.ident.to_string());
        self.scan.mods.push(declared);
        if m.content.is_some() {
            self.module.push(m.ident.to_string());
            syn::visit::visit_item_mod(self, m);
            self.module.pop();
        }
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if !is_cfg_test(&f.attrs) {
            syn::visit::visit_item_fn(self, f);
        }
    }

    fn visit_item_use(&mut self, u: &'ast syn::ItemUse) {
        if is_cfg_test(&u.attrs) {
            return;
        }
        let mut flat = Vec::new();
        flatten_use(&u.tree, &mut Vec::new(), &mut flat);
        let exported = !matches!(u.vis, syn::Visibility::Inherited);
        for (full, alias) in flat {
            if alias != "*" {
                self.scan
                    .locals
                    .insert((self.module.clone(), alias.clone()), full.clone());
            }
            if exported && alias != "*" {
                let mut a = self.module.clone();
                a.push(alias);
                self.scan
                    .reexports
                    .push((a, full.clone(), self.module.clone()));
            }
            self.scan.reaches.push((self.module.clone(), full));
        }
    }

    fn visit_path(&mut self, p: &'ast syn::Path) {
        let written: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
        self.scan.reaches.push((self.module.clone(), written));
        syn::visit::visit_path(self, p);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        for path in macro_paths(&m.tokens.to_string()) {
            self.scan.reaches.push((self.module.clone(), path));
        }
        syn::visit::visit_macro(self, m);
    }
}

fn scan_source(module: Vec<String>, text: &str) -> Scan {
    let file = syn::parse_file(text).expect("oxy-app source parses");
    let mut scan = Scan::default();
    Visitor {
        module,
        scan: &mut scan,
    }
    .visit_file(&file);
    scan
}

/// `name::rest` where `name` was imported in this module (or one enclosing
/// it in the same file) becomes the imported path followed by `rest`.
fn expand_local(
    locals: &BTreeMap<(Vec<String>, String), Vec<String>>,
    module: &[String],
    written: Vec<String>,
) -> Vec<String> {
    let Some(first) = written.first() else {
        return written;
    };
    if matches!(first.as_str(), "crate" | "self" | "super") {
        return written;
    }
    for n in (0..=module.len()).rev() {
        if let Some(full) = locals.get(&(module[..n].to_vec(), first.clone())) {
            if full.len() > 1 || full.first() != Some(first) {
                let mut out = full.clone();
                out.extend_from_slice(&written[1..]);
                return out;
            }
        }
    }
    written
}

/// Follow re-exports: an alias that names a lower crate drops out (`None`),
/// one that names another `oxy-app` module is replaced by it.
fn follow(
    path: Vec<String>,
    aliases: &BTreeMap<Vec<String>, Option<Vec<String>>>,
) -> Option<Vec<String>> {
    let mut path = path;
    for _ in 0..8 {
        let hit = (1..=path.len())
            .rev()
            .find_map(|n| aliases.get(&path[..n]).map(|t| (n, t)));
        match hit {
            None => return Some(path),
            Some((_, None)) => return None,
            Some((n, Some(target))) => {
                let mut next = target.clone();
                next.extend_from_slice(&path[n..]);
                if next == path {
                    return Some(path);
                }
                path = next;
            }
        }
    }
    Some(path)
}

/// Every reach from a base module out of the base: `(base entry, file, target)`.
fn base_reaches() -> Vec<(String, String, Vec<String>)> {
    let src = src_dir();
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let base: Vec<Vec<String>> = BASE.iter().map(|b| segs(b)).collect();

    let mut scanned = Vec::new();
    let mut modules = BTreeSet::new();
    for file in files.iter().filter(|f| !is_test_file(f)) {
        let text = std::fs::read_to_string(file).expect("read source");
        let module = file_module(&src, file);
        let scan = scan_source(module.clone(), &text);
        modules.insert(module);
        modules.extend(scan.mods.iter().cloned());
        scanned.push((file.clone(), scan));
    }
    let mut aliases: BTreeMap<Vec<String>, Option<Vec<String>>> = BTreeMap::new();
    for (_, scan) in &scanned {
        for (alias, target, module) in &scan.reexports {
            aliases.insert(alias.clone(), resolve(&modules, module, target));
        }
    }

    let mut out = Vec::new();
    for (file, scan) in scanned {
        for (module, written) in scan.reaches {
            let written = expand_local(&scan.locals, &module, written);
            let Some(from) = base.iter().find(|b| under(&module, b)) else {
                continue;
            };
            let Some(target) =
                resolve(&modules, &module, &written).and_then(|p| follow(p, &aliases))
            else {
                continue;
            };
            // A prefix of a base module (`crate::server`, `super::*` at a
            // parent) names no item outside it.
            if target.is_empty() || base.iter().any(|b| under(&target, b) || under(b, &target)) {
                continue;
            }
            let rel = file
                .strip_prefix(&src)
                .unwrap_or(&file)
                .display()
                .to_string();
            out.push((from.join("::"), rel, target));
        }
    }
    out
}

#[test]
fn base_reaches_out_only_through_the_backlog() {
    let mut fresh = BTreeSet::new();
    for (from, file, target) in base_reaches() {
        let listed = BACKLOG
            .iter()
            .any(|(b, t)| *b == from && under(&target, &segs(t)));
        if !listed {
            fresh.insert(format!("{from} -> {}   ({file})", target.join("::")));
        }
    }
    assert!(
        fresh.is_empty(),
        "a base module reaches a module that is not base, and the reach is not in \
         BACKLOG. The base is what leaves oxy-app downwards (domain-boundaries.md, \
         End state); each such import is one more thing it would have to take \
         along. Import from the base or a lower crate instead, or, if the target \
         is base too, add it to BASE:\n  {}",
        fresh.into_iter().collect::<Vec<_>>().join("\n  ")
    );
}

#[test]
fn every_backlog_row_is_still_a_reach() {
    let reaches = base_reaches();
    let stale: Vec<_> = BACKLOG
        .iter()
        .filter(|(b, t)| {
            !reaches
                .iter()
                .any(|(from, _, target)| from == b && under(target, &segs(t)))
        })
        .collect();
    assert!(
        stale.is_empty(),
        "BACKLOG rows that no longer match any reach: delete them, the list only \
         shrinks: {stale:?}"
    );
}

#[test]
fn every_base_module_exists() {
    let src = src_dir();
    let missing: Vec<_> = BASE
        .iter()
        .filter(|m| {
            let rel = m.replace("::", "/");
            !src.join(format!("{rel}.rs")).is_file() && !src.join(&rel).join("mod.rs").is_file()
        })
        .collect();
    assert!(
        missing.is_empty(),
        "BASE names modules that do not exist: {missing:?}. If one moved down, \
         delete its row."
    );
}

#[test]
fn the_scanner_reads_nested_uses_inline_modules_and_macros() {
    let text = r#"
        use crate::{a::{b, c::d}, e as f};
        mod inner {
            use super::super::g::h;
            fn x() { tracing::info!("{}", crate::m::n::CONST); }
        }
        #[cfg(test)]
        mod tests { use crate::t::u; }
        pub use oxy_shared::thing;
        mod child;
        pub use child::{inner_item, deeper::leaf};
        fn y() { child::call(); }
    "#;
    let scan = scan_source(segs("server::base"), text);
    let modules: BTreeSet<Vec<String>> = scan.mods.iter().cloned().collect();
    let reached: BTreeSet<String> = scan
        .reaches
        .iter()
        .filter_map(|(m, w)| resolve(&modules, m, w))
        .map(|p| p.join("::"))
        .collect();
    for want in [
        "a::b",
        "a::c::d",
        "e",
        "server::g::h",
        "m::n::CONST",
        "server::base::child::inner_item",
        "server::base::child::deeper::leaf",
        "server::base::child::call",
    ] {
        assert!(reached.contains(want), "missed {want}: {reached:?}");
    }
    assert!(
        !reached.iter().any(|p| p.starts_with("t::")),
        "scanned a cfg(test) module"
    );
    let reexport = &scan.reexports[0];
    assert_eq!(reexport.0, segs("server::base::thing"));
    assert_eq!(
        resolve(&modules, &reexport.2, &reexport.1),
        None,
        "a lower crate's re-export drops out"
    );
    // A re-export of a child module is followed into oxy-app, not mistaken
    // for an extern crate (which would drop every reach through it).
    let child = scan
        .reexports
        .iter()
        .find(|r| r.0 == segs("server::base::inner_item"))
        .expect("child re-export recorded");
    assert_eq!(
        resolve(&modules, &child.2, &child.1),
        Some(segs("server::base::child::inner_item"))
    );
}
