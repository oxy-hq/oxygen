//! Each test builds a throwaway workspace with path dependencies only, so
//! `cargo metadata --offline` resolves it without the network:
//!
//! ```text
//! core  <-  mid  <-  top        leaf (no dependents)   guard (always-test)
//! ```

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use guppy::graph::PackageGraph;
use guppy::graph::cargo::CargoResolverVersion;

use super::*;

const ROOT_MANIFEST: &str = r#"
[workspace]
members = ["core", "mid", "top", "leaf", "guard"]
resolver = "2"
"#;

fn workspace() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "test-selection-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    write(&dir, "Cargo.toml", ROOT_MANIFEST);
    for (name, deps) in [
        ("core", ""),
        ("mid", r#"core = { path = "../core" }"#),
        ("top", r#"mid = { path = "../mid" }"#),
        ("leaf", ""),
        ("guard", ""),
    ] {
        let manifest = format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{deps}\n"
        );
        write(&dir, &format!("{name}/Cargo.toml"), &manifest);
        write(&dir, &format!("{name}/src/lib.rs"), "");
    }
    dir
}

fn write(root: &Path, rel: &str, contents: &str) {
    let path = root.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn graph(dir: &Path) -> PackageGraph {
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--offline"])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    PackageGraph::from_json(String::from_utf8(out.stdout).unwrap()).unwrap()
}

fn config(text: &str) -> Config {
    Config::parse(text).unwrap()
}

fn run(dir: &Path, paths: &[&str], config: &Config) -> Result<Selection, String> {
    let g = graph(dir);
    let changed_paths: Vec<String> = paths.iter().map(|p| (*p).to_owned()).collect();
    let change = Change {
        old: &g,
        new: &g,
        changed_paths: &changed_paths,
        resolver: CargoResolverVersion::V2,
    };
    select(&change, config)
}

fn packages(names: &[&str]) -> Selection {
    Selection::Packages(
        names
            .iter()
            .map(|n| (*n).to_owned())
            .collect::<BTreeSet<_>>(),
    )
}

#[test]
fn a_change_selects_its_package_and_every_dependent() {
    let dir = workspace();
    let selection = run(&dir, &["core/src/lib.rs"], &config("")).unwrap();
    assert_eq!(selection, packages(&["core", "mid", "top"]));
}

#[test]
fn a_package_with_no_dependents_selects_only_itself() {
    let dir = workspace();
    let selection = run(&dir, &["leaf/src/lib.rs"], &config("")).unwrap();
    assert_eq!(selection, packages(&["leaf"]));
}

#[test]
fn always_test_rides_along_with_any_selection() {
    let dir = workspace();
    let cfg = config(r#"always-test = ["guard"]"#);
    let selection = run(&dir, &["leaf/src/lib.rs"], &cfg).unwrap();
    assert_eq!(selection, packages(&["guard", "leaf"]));
}

#[test]
fn a_path_no_package_owns_selects_everything() {
    let dir = workspace();
    let selection = run(&dir, &["fixtures/data.csv"], &config("")).unwrap();
    assert_eq!(selection, Selection::All);
}

#[test]
fn a_rule_can_route_an_outside_path_to_a_package() {
    let dir = workspace();
    let cfg = config(
        r#"
        [[determinator.path-rule]]
        globs = ["sdk/**"]
        mark-changed = ["mid"]
        "#,
    );
    let selection = run(&dir, &["sdk/src/index.ts"], &cfg).unwrap();
    assert_eq!(selection, packages(&["mid", "top"]));
}

#[test]
fn a_skip_rules_rule_marks_the_reader_and_still_the_owner() {
    let dir = workspace();
    let cfg = config(
        r#"
        [[determinator.path-rule]]
        globs = ["leaf/src/**"]
        mark-changed = ["core"]
        post-rule = "skip-rules"
        "#,
    );
    let selection = run(&dir, &["leaf/src/lib.rs"], &cfg).unwrap();
    assert_eq!(selection, packages(&["core", "leaf", "mid", "top"]));
}

#[test]
fn ignored_paths_select_nothing_and_skip_always_test() {
    let dir = workspace();
    let cfg = config(
        r#"
        always-test = ["guard"]
        [[determinator.path-rule]]
        globs = ["docs/**", "*.md"]
        mark-changed = []
        "#,
    );
    let selection = run(&dir, &["docs/guide.md", "CLAUDE.md", "Cargo.lock"], &cfg).unwrap();
    assert_eq!(selection, Selection::None);
}

#[test]
fn the_root_manifest_selects_everything() {
    let dir = workspace();
    let selection = run(&dir, &["Cargo.toml"], &config("")).unwrap();
    assert_eq!(selection, Selection::All);
}

#[test]
fn a_dependency_added_in_the_new_graph_marks_the_package() {
    let old_dir = workspace();
    let new_dir = workspace();
    write(
        &new_dir,
        "leaf/Cargo.toml",
        "[package]\nname = \"leaf\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncore = { path = \"../core\" }\n",
    );
    let (old, new) = (graph(&old_dir), graph(&new_dir));
    let change = Change {
        old: &old,
        new: &new,
        // Only the lockfile in the diff: the graph diff has to find `leaf`.
        changed_paths: &["Cargo.lock".to_owned()],
        resolver: CargoResolverVersion::V2,
    };
    assert_eq!(select(&change, &config("")).unwrap(), packages(&["leaf"]));
}

#[test]
fn an_unknown_always_test_package_is_an_error() {
    let dir = workspace();
    let err = run(
        &dir,
        &["leaf/src/lib.rs"],
        &config(r#"always-test = ["gone"]"#),
    )
    .unwrap_err();
    assert!(err.contains("gone"), "{err}");
}

#[test]
fn an_unknown_rule_package_is_an_error() {
    let dir = workspace();
    let cfg = config(
        r#"
        [[determinator.path-rule]]
        globs = ["sdk/**"]
        mark-changed = ["gone"]
        "#,
    );
    assert!(run(&dir, &["leaf/src/lib.rs"], &cfg).is_err());
}

#[test]
fn an_unknown_config_key_is_an_error() {
    assert!(Config::parse(r#"always-tested = ["guard"]"#).is_err());
}

#[test]
fn the_resolver_comes_from_the_root_manifest() {
    assert_eq!(
        workspace_resolver(ROOT_MANIFEST).unwrap(),
        CargoResolverVersion::V2
    );
    assert!(workspace_resolver("[workspace]\nmembers = []\n").is_err());
}

#[test]
fn the_selection_serializes_for_the_workflow() {
    let json = |s: &Selection| serde_json::to_string(s).unwrap();
    assert_eq!(json(&Selection::All), r#"{"mode":"all"}"#);
    assert_eq!(json(&Selection::None), r#"{"mode":"none"}"#);
    assert_eq!(
        json(&packages(&["b", "a"])),
        r#"{"mode":"packages","packages":["a","b"]}"#
    );
}

#[test]
fn the_repo_config_parses() {
    let text = include_str!("../../../../.config/test-selection.toml");
    let cfg = config(text);
    assert!(!cfg.always_test.is_empty());
}
