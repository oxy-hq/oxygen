//! Guard: a `reqwest::Client` is constructed in one place, `oxy_shared::http`.
//!
//! Same shape as `crates/app/tests/authz/authz_boundaries.rs`: a workspace-wide
//! walk, a list of banned shapes, and an `ALLOWED` backlog whose every entry
//! must still be needed. The walk reads the tree from `CARGO_MANIFEST_DIR/../..`
//! (the workspace root), skips test code — `tests/` directories, `*_tests.rs` /
//! `tests.rs` files and `#[cfg(test)]` modules — and does not need a database.

use std::fs;
use std::path::{Path, PathBuf};

/// The one file that may construct a `reqwest::Client`.
const FACTORY: &str = "crates/shared/src/http.rs";

/// Fully qualified constructions, called (`reqwest::Client::new()`) or passed as a
/// path (`get_or_init(reqwest::Client::new)`): the path must end at a token boundary.
const QUALIFIED: &[&str] = &[
    "reqwest::Client::new",
    "reqwest::Client::builder",
    "reqwest::ClientBuilder::new",
];

/// Constructions through an imported name — banned only in a file that
/// `use reqwest::…` imports `Client` or `ClientBuilder`, so `clickhouse::Client`
/// and `aws_sdk_s3::Client` sites are not misread, and only when the path starts
/// at a token boundary, so `clickhouse::Client::new()` beside a reqwest import and a
/// `MyClient::new()` are not either. Called or passed as a path alike.
const IMPORTED: &[&str] = &["Client::new", "Client::builder", "ClientBuilder::new"];

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Does `line` mention `path` as a whole token: not preceded by an identifier char
/// or `:` (another type's path), not followed by an identifier char? `()` after it
/// is a call; `)`, `,`, whitespace or line end is a path passed as a value.
fn mentions(line: &str, path: &str) -> bool {
    line.match_indices(path).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + path.len()..].chars().next();
        !before.is_some_and(|c| is_ident(c) || c == ':') && !after.is_some_and(is_ident)
    })
}

/// Sites permitted to build their own `reqwest::Client`, and why. Paths are
/// relative to the workspace root. **Shrinking this list is the migration.**
/// An entry nothing needs any more fails `every_allowlist_entry_is_still_needed`,
/// so it is removed in the change that made it unnecessary.
const ALLOWED: &[(&str, &str)] = &[];

/// Where this test may say "instead".
const DOOR: &str = "`oxy_shared::http::client()` for a plain request, or \
                    `oxy_shared::http::builder()` when the site needs its own timeout, \
                    headers, redirect policy or resolver (finish the build yourself)";

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/shared has a workspace root two levels up")
}

/// `path` relative to the workspace root, `/`-separated — the form `ALLOWED` uses.
fn rel_of(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/")
}

/// Is this a test file by path? `tests/` directories (integration groups) and
/// the `*_tests.rs` / `tests.rs` sibling-module convention.
fn is_test_path(path: &Path) -> bool {
    let in_tests_dir = path
        .components()
        .any(|c| c.as_os_str() == "tests" || c.as_os_str() == "benches");
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    in_tests_dir || stem == "tests" || stem.ends_with("_tests")
}

/// Every non-test `*.rs` under `crates/`, skipping build output and hidden dirs.
fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name == "target" || name.starts_with('.') || name == "node_modules" {
                continue;
            }
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") && !is_test_path(&path) {
            out.push(path);
        }
    }
}

/// The lines of `text` that are not inside a `#[cfg(test)] mod … { … }` block.
/// Brace-counted: good enough for a guard, and a miscount only ever surfaces as
/// a false positive on test code, which the failure message names.
fn non_test_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut armed = false; // saw `#[cfg(test)]`, waiting for the `mod … {`
    let mut depth = 0usize; // >0 while inside the skipped module
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if depth > 0 {
            depth += line.matches('{').count();
            depth = depth.saturating_sub(line.matches('}').count());
            continue;
        }
        if armed {
            if trimmed.starts_with("mod ") || trimmed.starts_with("pub mod ") {
                if trimmed.ends_with('{') {
                    depth = 1;
                } // `mod tests;` — a sibling file, skipped by path
                armed = false;
                continue;
            }
            if !trimmed.starts_with("#[") {
                armed = false; // the attribute belonged to a fn or a use
            }
        }
        if trimmed == "#[cfg(test)]" {
            armed = true;
            continue;
        }
        out.push((i + 1, line));
    }
    out
}

/// Does `text` import `Client` or `ClientBuilder` from reqwest? Reads every
/// `use reqwest::…;` item, multi-line braces included.
fn imports_reqwest_client(text: &str) -> bool {
    text.match_indices("use reqwest::").any(|(at, _)| {
        let item = &text[at..];
        let item = &item[..item.find(';').unwrap_or(item.len())];
        item.contains("Client")
    })
}

/// Banned construction sites in one file: `(line, text)`.
fn banned_sites(text: &str) -> Vec<(usize, String)> {
    let imported = imports_reqwest_client(text);
    non_test_lines(text)
        .into_iter()
        // A comment that names the shape is not a site.
        .filter(|(_, line)| !line.trim_start().starts_with("//"))
        .filter(|(_, line)| {
            QUALIFIED.iter().any(|n| mentions(line, n))
                || (imported && IMPORTED.iter().any(|n| mentions(line, n)))
        })
        .map(|(n, line)| (n, line.trim().to_string()))
        .collect()
}

fn sources() -> Vec<PathBuf> {
    let mut files = Vec::new();
    rust_sources(&workspace_root().join("crates"), &mut files);
    // A floor, not a target (~1500 today): a walk that silently scans nothing
    // is worse than no guard.
    assert!(
        files.len() > 500,
        "expected to scan every crate's sources, found {} files — the walk is broken",
        files.len()
    );
    files
}

#[test]
fn the_factory_is_the_only_place_a_reqwest_client_is_built() {
    let mut violations = Vec::new();
    for path in sources() {
        let rel = rel_of(&path);
        if rel == FACTORY || ALLOWED.iter().any(|(allowed, _)| rel.starts_with(allowed)) {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (line, found) in banned_sites(&text) {
            violations.push(format!("  {rel}:{line}\n    found: {found}"));
        }
    }
    assert!(
        violations.is_empty(),
        "\n{} site(s) build their own reqwest::Client:\n\n{}\n\n\
         Each one is its own connection pool, TLS cache, timeout policy and user agent. \
         Use {DOOR}. If the site really cannot (it is in a crate that may not depend on \
         oxy-shared), add it to ALLOWED in crates/shared/src/http_tests.rs with the reason.\n",
        violations.len(),
        violations.join("\n\n")
    );
}

/// An exemption nothing needs is an open door, and the list stops saying what
/// is left to migrate — so every entry must still cover a banned site.
#[test]
fn every_allowlist_entry_is_still_needed() {
    let files = sources();
    let stale: Vec<&str> = ALLOWED
        .iter()
        .map(|(allowed, _)| *allowed)
        .filter(|allowed| {
            !files.iter().any(|f| {
                rel_of(f).starts_with(allowed)
                    && fs::read_to_string(f).is_ok_and(|t| !banned_sites(&t).is_empty())
            })
        })
        .collect();
    assert!(
        stale.is_empty(),
        "ALLOWED names {} entr(ies) that no longer build a reqwest::Client: {stale:?}. \
         Remove them — an exemption nothing needs is one the next shortcut inherits.",
        stale.len()
    );
}

#[test]
fn the_factory_itself_is_seen_by_the_walk() {
    let factory = workspace_root().join(FACTORY);
    let text = fs::read_to_string(&factory).expect("http.rs exists");
    assert!(
        !banned_sites(&text).is_empty(),
        "the factory no longer constructs a client the way the guard recognises; \
         update QUALIFIED or the walk is guarding nothing"
    );
}

#[test]
fn test_modules_are_skipped() {
    let text = "fn a() {}\n#[cfg(test)]\nmod tests {\n    fn b() { let c = reqwest::Client::new(); }\n}\nfn d() {}\n";
    assert!(banned_sites(text).is_empty());
    let text = "#[cfg(test)]\nfn only_in_tests() { reqwest::Client::new(); }\nfn e() { reqwest::Client::new(); }\n";
    assert_eq!(
        banned_sites(text).len(),
        2,
        "a cfg(test) fn is not a module; both lines count"
    );
}

#[test]
fn a_path_passed_without_a_call_is_a_construction_too() {
    // `OnceLock::get_or_init(Client::new)` builds a client without a `()` in sight.
    assert_eq!(
        banned_sites("use reqwest::Client;\nSHARED.get_or_init(Client::new).clone()").len(),
        1
    );
    assert_eq!(
        banned_sites("CELL.get_or_init(reqwest::Client::new)").len(),
        1
    );
    assert_eq!(
        banned_sites("let make = reqwest::Client::builder;").len(),
        1
    );
    assert_eq!(
        banned_sites("let f: fn() -> Client = Client::new\n").len(),
        0,
        "no reqwest import"
    );
}

#[test]
fn comments_and_other_types_named_client_are_not_sites() {
    assert!(
        banned_sites("use reqwest::Client;\n// `Client::new()` builds a fresh pool\nlet c = 1;")
            .is_empty()
    );
    assert!(
        banned_sites("use reqwest::Client;\n/// Process-wide: `reqwest::Client::new()`\nfn f() {}")
            .is_empty()
    );
    assert!(banned_sites("use reqwest::Client;\nlet c = clickhouse::Client::new();").is_empty());
    assert!(banned_sites("use reqwest::Client;\nlet c = MyClient::new();").is_empty());
    assert!(banned_sites("use reqwest::Client;\nlet c = Client::new_with(x);").is_empty());
}

#[test]
fn imported_names_are_banned_only_with_a_reqwest_import() {
    assert!(
        banned_sites("use clickhouse::Client;\nlet c = Client::default();\nClient::new()")
            .is_empty()
    );
    assert_eq!(
        banned_sites(
            "use reqwest::{\n    Client,\n    header::USER_AGENT,\n};\nlet c = Client::builder();"
        )
        .len(),
        1
    );
    assert_eq!(
        banned_sites("use reqwest::Client;\nlet c = Client::new();").len(),
        1
    );
}

#[test]
fn defaults_are_what_the_docs_say() {
    assert!(super::USER_AGENT.starts_with("oxy/"));
    assert!(super::CONNECT_TIMEOUT < super::REQUEST_TIMEOUT);
    // Building the shared client cannot fail, and a second call is a clone of
    // the same configured client, not a second pool.
    let a = super::client();
    let b = super::client();
    assert_eq!(format!("{a:?}"), format!("{b:?}"));
    // The builder carries the defaults and finishes without an overall timeout.
    super::builder()
        .build()
        .expect("a builder with only the shared defaults is constructible");
}
