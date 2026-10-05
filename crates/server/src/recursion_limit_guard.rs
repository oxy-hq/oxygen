//! Every HTTP surface crate declares `#![recursion_limit = "256"]` at its root.
//!
//! A surface crate's handlers await `oxy-app`'s futures, and laying one of those
//! out can pass rustc's default query depth (128) in the optimized build only.
//! No PR job compiles with `--release`, so the first anyone hears of it is the
//! image build failing on main — which it did from 2026-10-02 to 2026-10-05,
//! after `oxy-api-source-upload` was split out of `oxy-app` and left `oxy-app`'s
//! limit behind (#3476). The attribute is per crate and has no workspace-wide
//! spelling, so the next crate split out loses it the same way.
//!
//! The crates are read off the filesystem rather than listed, so a surface
//! added tomorrow is covered the day it lands. This reads source text and
//! links nothing new: it rides in the test binary `served_router_tests` already
//! builds, in the one crate that depends on every surface.

use std::path::{Path, PathBuf};

/// The least a surface crate may declare: rustc's own suggestion, and what
/// every crate in this workspace that sets one uses.
const FLOOR: u32 = 256;

/// `source` with its comments removed: `//` to the end of the line, and
/// `/* … */`, which nests in Rust.
///
/// String literals are not understood, so a `/*` inside one swallows what
/// follows. That can hide an attribute but never invent one — the guard errs
/// toward failing.
fn without_comments(source: &str) -> String {
    let mut code = String::with_capacity(source.len());
    let mut depth = 0usize;
    let mut in_line_comment = false;
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' {
            in_line_comment = false;
            code.push(c);
            continue;
        }
        if in_line_comment {
            continue;
        }
        match (c, chars.peek().copied()) {
            ('/', Some('*')) => {
                chars.next();
                depth += 1;
            }
            ('*', Some('/')) if depth > 0 => {
                chars.next();
                depth -= 1;
            }
            ('/', Some('/')) if depth == 0 => in_line_comment = true,
            _ if depth == 0 => code.push(c),
            _ => {}
        }
    }
    code
}

/// The limit `source` declares as code: a line that begins
/// `#![recursion_limit = "N"]`. A `cfg_attr` does not count — the release
/// build is exactly the configuration that must not be left out.
fn declared_limit(source: &str) -> Option<u32> {
    without_comments(source).lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("#![recursion_limit")?;
        let rest = rest.trim_start().strip_prefix('=')?.trim_start();
        let (digits, rest) = rest.strip_prefix('"')?.split_once('"')?;
        rest.trim_start().strip_prefix(']')?;
        digits.parse().ok()
    })
}

/// What is wrong with a crate root, or `None` when it declares enough.
fn problem(source: Option<&str>) -> Option<String> {
    let Some(source) = source else {
        return Some("no crate root to read here".to_owned());
    };
    match declared_limit(source) {
        None => Some("declares none".to_owned()),
        Some(limit) if limit < FLOOR => Some(format!("declares {limit}")),
        Some(_) => None,
    }
}

/// The workspace root, from this crate's manifest at `crates/server`.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root is two levels above crates/server")
}

/// Every crate root that must declare it: each `crates/api-*` library, and this
/// binary, which is its own crate root and composes them all.
fn surface_roots(workspace: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = std::fs::read_dir(workspace.join("crates"))
        .expect("crates/ is readable")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("api-"))
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.path().join("src/lib.rs"))
        .collect();
    roots.push(workspace.join("crates/server/src/main.rs"));
    roots.sort();
    roots
}

#[test]
fn every_surface_crate_root_declares_the_recursion_limit() {
    let workspace = workspace();
    let roots = surface_roots(&workspace);
    // The scan is only worth anything if it can see a surface crate at all.
    assert!(
        roots.len() > 1,
        "found no crates/api-* under {}; this guard is matching nothing",
        workspace.display()
    );

    let offenders: Vec<String> = roots
        .iter()
        .filter_map(|root| {
            let source = std::fs::read_to_string(root).ok();
            let shown = root.strip_prefix(&workspace).unwrap_or(root).display();
            problem(source.as_deref()).map(|why| format!("  {shown} — {why}"))
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "these crate roots do not declare `#![recursion_limit = \"{FLOOR}\"]` or higher:\n\n{}\n\n\
         Add it to the crate root, as code — a comment does not count. A surface \
         crate's handlers await oxy-app's futures, and laying those out can pass \
         rustc's default query depth of 128 in the `--release` build only. No PR \
         job compiles in release, so the first sign is the image build failing on \
         main, where it stopped every deploy for three days: \
         https://github.com/oxy-hq/oxygen-internal/pull/3476",
        offenders.join("\n")
    );
}

#[test]
fn only_an_attribute_written_as_code_counts() {
    let declared = "//! Docs.\n\n// Why.\n#![recursion_limit = \"256\"]\n\npub mod a;\n";
    assert_eq!(declared_limit(declared), Some(256));
    assert_eq!(problem(Some(declared)), None);
    assert_eq!(problem(Some("#![recursion_limit = \"512\"]\n")), None);

    for (what, source) in [
        ("no attribute", "//! Docs.\npub mod a;\n"),
        (
            "commented out",
            "// #![recursion_limit = \"256\"]\npub mod a;\n",
        ),
        (
            "in a doc comment",
            "//! #![recursion_limit = \"256\"]\npub mod a;\n",
        ),
        (
            "in a block comment",
            "/*\n#![recursion_limit = \"256\"]\n*/\npub mod a;\n",
        ),
        (
            "in a nested block comment",
            "/* a /* b */\n#![recursion_limit = \"256\"]\n*/\npub mod a;\n",
        ),
        (
            "behind a cfg_attr",
            "#![cfg_attr(not(debug_assertions), recursion_limit = \"256\")]\n",
        ),
    ] {
        assert_eq!(
            problem(Some(source)).as_deref(),
            Some("declares none"),
            "{what}"
        );
    }

    assert_eq!(
        problem(Some("#![recursion_limit = \"128\"]\n")).as_deref(),
        Some("declares 128"),
        "the default, spelled out, is not a raise"
    );
    assert_eq!(
        problem(None).as_deref(),
        Some("no crate root to read here"),
        "an api-* directory with no src/lib.rs is reported, not skipped"
    );
}
