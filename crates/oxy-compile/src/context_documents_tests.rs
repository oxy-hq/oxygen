//! Tests for [`super`]. Split out to keep the module under the file-size cap.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;
use crate::walker::discover as discover_workspace;

fn write(root: &Path, rel: &str, body: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// What `patterns` reach under `root`, in the order `discover` returns them.
fn reached(root: &Path, patterns: &[&str]) -> Vec<String> {
    discover(root, &ContextPatterns::new(patterns))
        .expect("the root is readable")
        .into_iter()
        .map(|file| file.rel_path)
        .collect()
}

/// The resolution the analytics run used before documents were compiled:
/// `glob::glob` under the root, keeping regular files whose extension is `md`.
fn legacy(root: &Path, pattern: &str) -> Vec<String> {
    let absolute = root.join(pattern).to_string_lossy().into_owned();
    let mut found: Vec<String> = glob::glob(&absolute)
        .expect("the pattern parses")
        .flatten()
        .filter(|path| path.is_file() && path.extension().is_some_and(|ext| ext == "md"))
        .map(|path| {
            path.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    found.sort();
    found.dedup();
    found
}

fn workspace() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "README.md", "# readme");
    write(root, "docs/glossary.md", "# glossary");
    write(root, "docs/metrics.md", "# metrics");
    write(root, "docs/notes.txt", "not markdown");
    write(
        root,
        "docs/.draft.md",
        "# a dot-prefixed FILE is still a file",
    );
    write(root, "docs/sub/deeper.md", "# deeper");
    write(root, "semantics/orders.view.yml", "name: orders");
    write(root, "semantics/notes.md", "# notes beside the views");
    write(root, "nested/a/deep.md", "# a");
    write(root, "nested/b/deep.md", "# b");
    write(root, "nested/b/c/deep.md", "# c");
    dir
}

/// The central claim about the matcher: for a pattern that stays inside the
/// workspace, it reaches exactly the markdown `glob::glob` reached. Every row
/// is a shape an agent's `context:` uses or plausibly could.
#[test]
fn a_pattern_reaches_what_glob_reached_when_the_run_read_the_disk() {
    let dir = workspace();
    let root = dir.path();

    for pattern in [
        "./docs/*.md",
        "docs/*.md",
        "./docs/**/*.md",
        "./docs/**/*",
        "**/*.md",
        "./**/*",
        "*",
        "*.md",
        "docs/glossary.md",
        "./docs/glossary.md",
        "docs/*",
        "docs/**",
        "docs",
        "docs/",
        "./docs/g*.md",
        "docs/?lossary.md",
        "docs/[gm]*.md",
        "./semantics/**/*",
        "./semantics/*.view.yml",
        "nested/*/deep.md",
        "**/deep.md",
        "./nested/**/c/*.md",
        "./sem*/**/*.md",
        "./missing/*.md",
    ] {
        let mut ours = reached(root, &[pattern]);
        ours.sort();
        assert_eq!(
            ours,
            legacy(root, pattern),
            "`{pattern}` reaches a different set of documents than `glob::glob` did"
        );
    }
}

/// The parity test above would pass with both sides empty. This is the floor
/// under it: the fixture's shapes do match, in the numbers expected.
#[test]
fn the_parity_fixture_is_not_vacuous() {
    let dir = workspace();
    let root = dir.path();

    assert_eq!(
        reached(root, &["./docs/*.md"]),
        ["docs/.draft.md", "docs/glossary.md", "docs/metrics.md"]
    );
    assert_eq!(reached(root, &["**/*.md"]).len(), 9);
    assert_eq!(
        reached(root, &["./semantics/**/*"]),
        ["semantics/notes.md"],
        "a broad glob over the semantic model also reaches markdown beside it"
    );
    assert!(reached(root, &["docs/**"]).is_empty());
}

/// A revision cannot hold a file from outside the workspace, so a pattern that
/// names one reaches nothing — where it used to read the host's filesystem.
#[test]
fn a_pattern_that_leaves_the_workspace_reaches_nothing() {
    let outer = TempDir::new().unwrap();
    write(outer.path(), "secret.md", "# not this workspace's");
    write(outer.path(), "ws/docs/glossary.md", "# glossary");
    let root = outer.path().join("ws");

    let absolute = format!("{}/*.md", outer.path().display());
    for pattern in ["../*.md", "./docs/../../*.md", absolute.as_str()] {
        assert!(
            reached(&root, &[pattern]).is_empty(),
            "`{pattern}` reached a file outside the workspace"
        );
        assert!(!ContextPatterns::new([pattern]).may_reach_documents());
    }
    assert_eq!(
        reached(&root, &["../*.md", "./docs/*.md"]),
        ["docs/glossary.md"],
        "a refused pattern does not take its neighbours with it"
    );
}

/// Both workspace skip rules, which every compiled kind inherits: a skipped
/// directory at any depth, and a `.test.` file NAME (never a directory).
#[test]
fn the_two_workspace_skip_rules_apply_to_documents() {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    write(root, "docs/kept.md", "# kept");
    write(
        root,
        "docs/v1.test.cases/kept-too.md",
        "# a fixtures DIRECTORY",
    );
    write(root, "docs/draft.test.md", "# a fixture FILE");
    write(root, "node_modules/pkg/README.md", "# vendored");
    write(
        root,
        "docs/node_modules/pkg/README.md",
        "# vendored, nested",
    );
    write(root, ".git/description.md", "# hidden");
    write(root, "sub/target/doc.md", "# build output");
    write(root, "dist/doc.md", "# build output");
    write(root, "sub/build/doc.md", "# build output");

    assert_eq!(
        reached(root, &["./**/*.md"]),
        ["docs/kept.md", "docs/v1.test.cases/kept-too.md"]
    );
}

/// The order a run injects documents in is the agent's: pattern by pattern,
/// then by path. A file two patterns reach is listed once, under the first.
#[test]
fn documents_are_ordered_by_pattern_then_path_and_listed_once() {
    let dir = workspace();

    assert_eq!(
        reached(
            dir.path(),
            &["./nested/**/*.md", "./docs/*.md", "**/deep.md"]
        ),
        [
            "nested/a/deep.md",
            "nested/b/c/deep.md",
            "nested/b/deep.md",
            "docs/.draft.md",
            "docs/glossary.md",
            "docs/metrics.md",
        ]
    );
}

/// Whether a pattern list could name a `.md` at all. A wrong `false` here
/// would silently drop an agent's documents, so every `false` row is one where
/// the name is pinned to another ending.
#[test]
fn only_a_pattern_pinned_to_another_ending_cannot_name_a_document() {
    for (pattern, expected) in [
        ("./docs/*.md", true),
        ("./docs/glossary.md", true),
        ("./semantics/**/*", true),
        ("./**/*", true),
        ("*", true),
        ("docs/*d", true),
        ("docs/*.m?", true),
        ("docs/notes.[mt]d", true),
        ("docs/[gm]*", true),
        ("./semantics/*.view.yml", false),
        ("./example_sql/*.sql", false),
        ("./workflows/**/*.automation.yml", false),
        ("docs/glossary.txt", false),
        ("docs/**", false),
        ("docs", false),
    ] {
        assert_eq!(
            ContextPatterns::new([pattern]).may_reach_documents(),
            expected,
            "`{pattern}`"
        );
    }
    assert!(!ContextPatterns::new(Vec::<String>::new()).may_reach_documents());
    assert!(
        ContextPatterns::new(["./semantics/*.view.yml", "./docs/*.md"]).may_reach_documents(),
        "one pattern that can is enough"
    );
}

/// A pattern that does not parse is dropped here and reported by the run's
/// own resolution of the same string, which names it.
#[test]
fn a_pattern_that_does_not_parse_reaches_nothing() {
    let dir = workspace();
    assert!(reached(dir.path(), &["docs/[.md"]).is_empty());
    assert!(reached(dir.path(), &["docs/a**.md"]).is_empty());
}

/// The one decoding both arms use. Nothing in it can fail, which is the point:
/// a byte Postgres cannot store must not be a reason a workspace cannot promote.
#[test]
fn document_text_keeps_everything_a_revision_can_hold() {
    assert_eq!(document_text(b"# Glossary\n"), "# Glossary\n");
    assert_eq!(document_text(b"a\0b\0"), "ab", "NUL bytes are dropped");
    assert_eq!(
        document_text(b"caf\xff\0"),
        "caf\u{fffd}",
        "invalid UTF-8 is replaced, as the compiler has always done for every file"
    );
}

/// "Could not look" is not "found none".
#[test]
fn a_root_that_is_not_there_is_an_error_not_an_empty_list() {
    let dir = TempDir::new().unwrap();
    let absent = dir.path().join("never-cloned-here");

    assert!(discover(&absent, &ContextPatterns::new(["./docs/*.md"])).is_err());
    assert!(
        !absent.exists(),
        "and looking must not bring the root into existence"
    );
}

#[cfg(unix)]
#[test]
fn a_directory_symlink_is_not_followed() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("ws");
    write(&root, "docs/glossary.md", "# glossary");
    write(
        dir.path(),
        "elsewhere/outside.md",
        "# outside the workspace",
    );
    std::os::unix::fs::symlink(dir.path().join("elsewhere"), root.join("linked")).unwrap();
    // A cycle would hang the walk if it were followed.
    std::os::unix::fs::symlink(&root, root.join("docs/loop")).unwrap();

    assert_eq!(reached(&root, &["./**/*.md"]), ["docs/glossary.md"]);
}

fn documents(root: &Path) -> Vec<String> {
    discover_workspace(root)
        .expect("the workspace walks")
        .into_iter()
        .filter(|file| file.kind == FileKind::ContextDocument)
        .map(|file| file.rel_path)
        .collect()
}

/// What a compile carries: the union of what every agent reaches, and nothing
/// else. The README nobody references is the case the kind exists to exclude.
#[test]
fn a_compile_carries_the_documents_agents_reach_and_no_others() {
    let dir = workspace();
    let root = dir.path();
    write(
        root,
        "analyst.agentic.yml",
        "name: analyst\ncontext:\n  - ./semantics/**/*\n  - ./docs/*.md\n",
    );
    // Patterns resolve from the workspace root wherever the agent file sits.
    write(
        root,
        "agents/deep.agentic.yml",
        "name: deep\ncontext:\n  - ./nested/b/**/*.md\n",
    );

    assert_eq!(
        documents(root),
        [
            "docs/.draft.md",
            "docs/glossary.md",
            "docs/metrics.md",
            "nested/b/c/deep.md",
            "nested/b/deep.md",
            "semantics/notes.md",
        ],
        "sorted by path like the rest of the walk, README.md and docs/sub/ left out"
    );
}

#[test]
fn a_workspace_whose_agents_reach_no_markdown_compiles_none() {
    let dir = workspace();
    let root = dir.path();
    assert!(documents(root).is_empty(), "no agents, no documents");

    write(
        root,
        "analyst.agentic.yml",
        "name: analyst\ncontext:\n  - ./semantics/*.view.yml\n",
    );
    write(root, "bare.agentic.yml", "name: bare\n");
    // Not YAML at all. It fails the compile as an agent; here it only must
    // not stop the walk or invent patterns.
    write(root, "broken.agentic.yml", "name: [unterminated\n");
    // `context:` in a shape the agent schema rejects.
    write(root, "odd.agentic.yml", "name: odd\ncontext: ./docs/*.md\n");

    assert!(documents(root).is_empty());
}
