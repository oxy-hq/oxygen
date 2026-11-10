//! Surface crates import `oxy-app` through `oxy_app::surface` only, and `oxy-app`
//! holds no custom-app module.
//!
//! Rule S4 in `internal-docs/domain-boundaries.md`. `crates/app/src/surface.rs`
//! is the contract between the platform runtime and the extracted HTTP surfaces
//! (`crates/api-*`): guards, org/workspace context, session helpers, route
//! roles. Anything else a surface takes from `oxy-app` is another bounded
//! context's logic, which rule S3 sends down into a domain crate instead.
//!
//! [`BACKLOG`] is the list of those reaches that predate the rule — it is a
//! backlog, not an exemption list (the same stance as `authz_boundaries.rs`).
//! A new reach fails here; so does a backlog entry that no longer matches
//! anything, so the list can only shrink. Entries match whole path segments,
//! so `server::api::admin::apps` does not also admit `server::api::admin::assume`
//! (which the prelude now provides). Aliasing the crate (`use oxy_app as x`,
//! `extern crate oxy_app`) is refused outright: it would hide every later
//! path from the scan.
//!
//! ## The custom-app vertical is out of `oxy-app`
//!
//! This file also holds what `tests/custom_apps/custom_apps_boundary.rs` used to:
//! that test kept the custom-app modules behind a named list of seams into the rest
//! of `oxy-app`, by scanning `crate::` imports. The modules are in
//! `oxy-api-custom-apps` now, so their reach into `oxy-app` is an `oxy_app::` path,
//! which the first test below holds to the prelude, and the second test keeps the
//! vertical from growing back: `oxy-app` has no `custom_app*` module that is not a
//! re-export of one that moved, except the one named in [`NOT_SHIMS`]. The third test
//! below is stricter: `crates/app/src` has no `custom_apps_*` file or directory at all
//! except `custom_apps_gates.rs`.

use std::path::{Path, PathBuf};

/// `(surface crate dir, oxy_app path prefix)` — reaches into another context,
/// each waiting on the extraction named beside it (Pending list in
/// `domain-boundaries.md`).
const BACKLOG: &[(&str, &str)] = &[
    // custom-apps vertical
    // tenancy (api-tenancy)
    // write_access flushes oxy-app's custom-app caches; the rest is oxy-tenancy
    (
        "api-tenancy",
        "server::api::org_teams::service::write_access",
    ),
    // billing: seat sync when a member is added or removed
    ("api-tenancy", "server::api::billing::billing_service"),
    // workspace context: schedule cleanup on delete, the directory read, logo bytes
    ("api-tenancy", "server::api::workspaces"),
    ("api-tenancy", "server::api::workspace_logo"),
    // notifications: the Slack DM channel links a person to their Slack user by
    // email and reads the org's installation (the bot token). Slack is its own
    // context still inside oxy-app; these clear when it leaves.
    (
        "api-notifications",
        "integrations::slack::linking::by_email",
    ),
    (
        "api-notifications",
        "integrations::slack::services::installations",
    ),
    // and its integration test, which seeds a person's existing Slack link
    (
        "api-notifications",
        "integrations::slack::services::user_links",
    ),
];

/// `path` is `prefix` or lies under it — on a segment boundary, so
/// `org_teams` does not admit `org_teams_foo`.
fn under(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with("::"))
}

/// A line that renames the crate, after which `oxy_app::` never appears.
fn aliases_the_crate(code: &str) -> bool {
    let words: Vec<&str> = code
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .collect();
    words
        .windows(2)
        .any(|w| (w[0] == "oxy_app" && w[1] == "as") || (w[0] == "crate" && w[1] == "oxy_app"))
}

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
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

/// Every `oxy_app::<path>` in a crate's code (comments skipped), as
/// `(file, line, path-after-oxy_app::)`.
fn reaches(crate_dir: &Path) -> Vec<(String, usize, String)> {
    let mut files = Vec::new();
    rust_files(&crate_dir.join("src"), &mut files);
    rust_files(&crate_dir.join("tests"), &mut files);
    let mut found = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file).unwrap_or_default();
        for (i, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or_default();
            if aliases_the_crate(code) {
                found.push((file.display().to_string(), i + 1, "<alias>".to_string()));
            }
            let mut rest = code;
            while let Some(at) = rest.find("oxy_app::") {
                let tail = &rest[at + "oxy_app::".len()..];
                let path: String = tail
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == ':' || *c == '{')
                    .collect();
                found.push((file.display().to_string(), i + 1, path));
                rest = tail;
            }
        }
    }
    found
}

fn surface_crates() -> Vec<(String, PathBuf)> {
    let mut out: Vec<_> = std::fs::read_dir(crates_dir())
        .expect("crates/ is readable")
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            name.starts_with("api-").then(|| (name, e.path()))
        })
        .collect();
    out.sort();
    out
}

#[test]
fn surfaces_reach_oxy_app_only_through_the_prelude() {
    let crates = surface_crates();
    // The scan is pointed at the right place if it finds every crate the
    // backlog names. Not a count: surfaces merge (onboarding + partner console
    // became `api-tenancy`), and a floor would fail each time one does.
    for (name, _) in BACKLOG {
        assert!(
            crates.iter().any(|(n, _)| n == name),
            "the scan found no `crates/{name}` although BACKLOG names it — the walk \
             lost its way, or the crate moved and BACKLOG still has its old name"
        );
    }

    let mut violations = Vec::new();
    for (name, dir) in &crates {
        for (file, line, path) in reaches(dir) {
            if under(&path, "surface") {
                continue;
            }
            let backlogged = BACKLOG
                .iter()
                .any(|(c, prefix)| c == name && under(&path, prefix));
            if !backlogged {
                violations.push(format!("{file}:{line}: oxy_app::{path}"));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "a surface crate reaches into oxy-app outside `oxy_app::surface` \
         (internal-docs/domain-boundaries.md S3/S4). If it is platform runtime, \
         re-export it from crates/app/src/surface.rs; if it is another context's \
         logic, lower it into a domain crate — do not add it to BACKLOG:\n  {}",
        violations.join("\n  ")
    );
}

#[test]
fn backlog_entries_match_whole_segments_and_aliases_are_caught() {
    assert!(under(
        "server::api::admin::apps::handlers",
        "server::api::admin::apps"
    ));
    assert!(!under(
        "server::api::admin::assume",
        "server::api::admin::apps"
    ));
    assert!(!under(
        "server::api::org_teams_foo",
        "server::api::org_teams"
    ));
    assert!(!under("surface_x::y", "surface"));
    assert!(aliases_the_crate("use oxy_app as app;"));
    assert!(aliases_the_crate("extern crate oxy_app;"));
    assert!(!aliases_the_crate("use oxy_app::surface::roles;"));
}

#[test]
fn every_backlog_entry_is_still_a_real_reach() {
    let crates = surface_crates();
    let stale: Vec<String> = BACKLOG
        .iter()
        .filter(|(name, prefix)| {
            let Some((_, dir)) = crates.iter().find(|(n, _)| n == name) else {
                return true;
            };
            !reaches(dir).iter().any(|(_, _, p)| under(p, prefix))
        })
        .map(|(name, prefix)| format!("{name}: oxy_app::{prefix}"))
        .collect();
    assert!(
        stale.is_empty(),
        "these BACKLOG entries no longer match any import — the reach was \
         removed, so delete the entry:\n  {}",
        stale.join("\n  ")
    );
}

/// `custom_app*` files and directories `oxy-app` still has that are not a
/// re-export, each with why. **Not a backlog to grow.**
const NOT_SHIMS: &[(&str, &str)] = &[(
    "server/api/custom_apps_gates.rs",
    "the data plane's gate (`check_custom_app_gates` answered as a response, \
     `parse_versioned_body`) and the project-context composition root \
     (`build_project_context*`), which `projects::{query, semantic_query, agent_ask, \
     automation_run, ...}` call and which build an `OxyProjectContext`. The decisions \
     are `oxy_custom_apps::gate`; the function host reaches the context builder \
     through `FunctionQueryExecutor::build_project_context`.",
)];

fn src_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            src_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Whether a file is only comments, blank lines, `use`/`pub use` items and the
/// attributes and `mod` lines that carry them.
fn is_reexport_only(text: &str) -> bool {
    let mut in_use = false;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with("//") {
            continue;
        }
        if in_use {
            in_use = !t.contains(';');
            continue;
        }
        if t.starts_with("pub use ") || t.starts_with("pub(crate) use ") || t.starts_with("use ") {
            in_use = !t.contains(';');
            continue;
        }
        if t.starts_with("#[") {
            continue;
        }
        return false;
    }
    true
}

#[test]
fn oxy_app_holds_no_custom_app_module_except_shims() {
    let app_src = crates_dir().join("app").join("src");
    let mut files = Vec::new();
    src_files(&app_src, &mut files);
    assert!(
        files.len() > 200,
        "the walk found only {} files under crates/app/src — it lost its way",
        files.len()
    );
    let mut offenders = Vec::new();
    for file in &files {
        let rel = file
            .strip_prefix(&app_src)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        let named = rel
            .split('/')
            .any(|seg| seg.starts_with("custom_app") || seg == "workspace_custom_apps.rs");
        if !named || !rel.ends_with(".rs") {
            continue;
        }
        if NOT_SHIMS.iter().any(|(path, _)| *path == rel) {
            continue;
        }
        let text = std::fs::read_to_string(file).unwrap_or_default();
        if !is_reexport_only(&text) {
            offenders.push(rel);
        }
    }
    assert!(
        offenders.is_empty(),
        "`oxy-app` holds custom-app code again. The custom-app vertical lives in \
         `crates/api-custom-apps` (or `oxy-custom-apps` / `oxy-app-core` when `oxy-app` \
         also uses it); a file here may only re-export a moved module:\n  {}",
        offenders.join("\n  ")
    );
    for (path, _) in NOT_SHIMS {
        assert!(
            app_src.join(path).exists(),
            "NOT_SHIMS names `{path}`, which is gone — delete the entry"
        );
    }
}

/// Every file or directory name under `crates/app/src` that starts with
/// `custom_apps_`, stripped of `.rs`, as a path relative to `src`.
fn custom_apps_named_modules(app_src: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    src_entries(app_src, &mut paths);
    paths
        .iter()
        .filter_map(|p| {
            let name = p.file_name()?.to_string_lossy();
            let stem = name.strip_suffix(".rs").unwrap_or(&name);
            stem.starts_with("custom_apps_").then(|| {
                p.strip_prefix(app_src)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/")
            })
        })
        .collect()
}

/// Files and directories both, so a `custom_apps_x/` directory is seen even
/// when its `mod.rs` is the only file in it.
fn src_entries(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        out.push(path.clone());
        if path.is_dir() {
            src_entries(&path, out);
        }
    }
}

#[test]
fn oxy_app_has_no_custom_apps_module_except_gates() {
    let app_src = crates_dir().join("app").join("src");
    let found = custom_apps_named_modules(&app_src);
    let offenders: Vec<&String> = found
        .iter()
        .filter(|rel| rel.as_str() != "server/api/custom_apps_gates.rs")
        .collect();
    assert!(
        offenders.is_empty(),
        "`crates/app/src` has a `custom_apps_*` module besides `custom_apps_gates`. \
         The last re-export files were retired; import from the crate that owns the \
         name (`oxy_custom_apps`, `oxy_app_core`) instead of adding one back:\n  {}",
        offenders
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n  ")
    );
    assert!(
        found
            .iter()
            .any(|rel| rel == "server/api/custom_apps_gates.rs"),
        "the walk did not find `custom_apps_gates.rs` — it lost its way"
    );
}

#[test]
fn surface_sources_do_not_take_the_test_fixtures() {
    // `oxy_app::surface::testing` is for integration tests; a surface's `src` that
    // took from it would make the data plane's handlers part of its contract.
    let mut found = Vec::new();
    for (name, dir) in surface_crates() {
        let mut files = Vec::new();
        rust_files(&dir.join("src"), &mut files);
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            for (i, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or_default();
                if code.contains("surface::testing") {
                    found.push(format!("{name}: {}:{}", file.display(), i + 1));
                }
            }
        }
    }
    assert!(
        found.is_empty(),
        "src takes `surface::testing`:\n  {}",
        found.join("\n  ")
    );
}

#[test]
fn no_surface_crate_depends_on_another_surface_crate() {
    // Rule S3: a surface that needs another surface's logic gets it from a crate
    // below both, or through a port the composition root fills (`AppLifecycle`
    // is one). Not even a dev-dependency: a test that needs both is
    // `oxy-server`'s.
    let mut offenders = Vec::new();
    for (name, dir) in surface_crates() {
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
        for (i, line) in manifest.lines().enumerate() {
            let code = line.split('#').next().unwrap_or_default().trim();
            let Some((dep, _)) = code.split_once('=') else {
                continue;
            };
            let dep = dep.trim();
            if dep.starts_with("oxy-api-") && dep != format!("oxy-{name}") {
                offenders.push(format!("{name}: Cargo.toml:{} depends on {dep}", i + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "a surface crate depends on another surface crate (internal-docs/domain-boundaries.md S3):\n  {}",
        offenders.join("\n  ")
    );
}
