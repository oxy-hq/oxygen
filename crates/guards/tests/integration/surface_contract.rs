//! The rules between surface crates (`crates/api-*`) and the rest of the
//! workspace that a manifest or source scan can state: no surface takes
//! another surface (S3), none takes `oxy-server` outside its tests (S3, S5), and
//! none takes the test fixtures in `src`. Rules in
//! `internal-docs/domain-boundaries.md`.
//!
//! Moved from `oxy-app`'s `tests/routing/` to `oxy-guards` in decompose GRD
//! (2026-11-24). The five tests that guarded `oxy-app` itself — that surfaces
//! reach it only through the prelude (its `BACKLOG` and the segment and alias
//! checks), and that it holds no custom-app module (with the walk floor over
//! `crates/app/src`) — were deleted then with the user's approval: `oxy-app` is
//! being deleted, and no surface crate names it.

use std::path::{Path, PathBuf};

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

/// `Cargo.toml` lines naming `oxy-server` outside `[dev-dependencies]`.
fn oxy_server_outside_dev_deps(manifest: &str) -> Vec<String> {
    let mut section = "";
    let mut found = Vec::new();
    for (i, line) in manifest.lines().enumerate() {
        let code = line.split('#').next().unwrap_or_default().trim();
        if code.starts_with('[') {
            section = code;
        } else if code
            .split_once('=')
            .is_some_and(|(dep, _)| dep.trim() == "oxy-server")
            && section != "[dev-dependencies]"
        {
            found.push(format!("Cargo.toml:{} under {section}", i + 1));
        }
    }
    found
}

/// `file:line` for every non-comment line under `src` that names `oxy_server::`.
fn oxy_server_in_src(src: &Path) -> Vec<String> {
    let mut files = Vec::new();
    rust_files(src, &mut files);
    files
        .iter()
        .flat_map(|file| {
            let text = std::fs::read_to_string(file).unwrap_or_default();
            text.lines()
                .enumerate()
                .filter(|(_, line)| {
                    line.split("//")
                        .next()
                        .unwrap_or_default()
                        .contains("oxy_server::")
                })
                .map(|(i, _)| format!("{}:{}", file.display(), i + 1))
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The router a surface's tests drive is `oxy-server`'s since decompose LIFT
/// (2026-11-23), so `oxy-api-custom-apps` and `oxy-api-experiment` take it as a
/// dev-dependency. Only that: `oxy-server` mounts every surface, so a normal
/// dependency is a cycle Cargo refuses anyway, and the rule is stated here so
/// the failure names it. And never in `src`, where it would make the
/// composition root part of a surface's contract.
#[test]
fn surfaces_take_oxy_server_only_as_a_dev_dependency() {
    let mut offenders = Vec::new();
    for (name, dir) in surface_crates() {
        let manifest = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
        let hits = oxy_server_outside_dev_deps(&manifest)
            .into_iter()
            .chain(oxy_server_in_src(&dir.join("src")));
        offenders.extend(hits.map(|hit| format!("{name}: {hit}")));
    }
    assert!(
        offenders.is_empty(),
        "a surface takes oxy-server outside its tests (internal-docs/domain-boundaries.md S3, S5):\n  {}",
        offenders.join("\n  ")
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
