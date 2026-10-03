//! Surface crates import `oxy-app` through `oxy_app::surface` only.
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

use std::path::{Path, PathBuf};

/// `(surface crate dir, oxy_app path prefix)` — reaches into another context,
/// each waiting on the extraction named beside it (Pending list in
/// `domain-boundaries.md`).
const BACKLOG: &[(&str, &str)] = &[
    // custom-apps vertical
    ("api-documents", "server::api::custom_apps_gates"),
    ("api-documents", "server::api::custom_apps_storage"),
    ("api-frontline", "server::api::custom_apps_auth"),
    ("api-tenancy", "server::api::custom_apps_auth"),
    ("api-tenancy", "server::api::custom_apps_publish_authz"),
    // frontline completion: this logic belongs IN api-frontline
    ("api-frontline", "server::api::frontline_grants"),
    ("api-frontline", "server::api::frontline_admin"),
    // tenancy (api-tenancy) and the operating graph
    ("api-frontline", "server::api::operating_graph"),
    // write_access flushes oxy-app's custom-app caches; the rest is oxy-tenancy
    (
        "api-tenancy",
        "server::api::org_teams::service::write_access",
    ),
    ("api-tenancy", "server::api::organizations"),
    ("api-tenancy", "server::api::admin::apps"),
    ("api-tenancy", "server::api::admin::WorkspaceHealthRow"),
    ("api-tenancy", "server::api::admin::health_rollup"),
    ("api-tenancy", "server::service::workspace_provisioning"),
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
