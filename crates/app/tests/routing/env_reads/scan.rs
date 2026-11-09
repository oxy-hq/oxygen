//! The read-site scan, a port of `scripts/env-inventory.py`'s. The two must
//! agree: `inventory_script_agrees_with_this_scan` (in `mod.rs`) runs the
//! script's `--sites` and compares. A rule changed here is changed there.
//!
//! What counts as a read site, in non-test code under `crates/**/src/**/*.rs`:
//!
//! 1. `env::var("OXY_X")` / `env::var_os("OXY_X")`, with or without `std::`.
//! 2. A call to a function whose name contains `env` or `var` with an `"OXY_X"`
//!    literal as its first argument (`env_usize("OXY_X", 8)`, `require_var(..)`).
//! 3. Either through a named constant (`const GATE: &str = "OXY_X"` then
//!    `env::var(GATE)`), and a constant declared in any file that reads the
//!    environment at all — a gate read through a closure is still that file's.
//!
//! 4. In the central config module (`crates/app-core/src/config/`) every
//!    `"OXY_X"` literal: that module exists to read them, through a lookup.
//!
//! Test code is left out: `*_tests.rs` / `*_test.rs` / `tests.rs` files and
//! every item under a `#[cfg(test)]` (or `cfg(any(test, ..))`) attribute.
//! Strings and comments are lexed first so a brace in a string cannot
//! unbalance the item skip.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

const VAR: &str = r"OXY_[A-Z0-9_]+";
/// Where every literal is a read (rule 4).
pub const CENTRAL: &str = "crates/app-core/src/config/";

static LITERAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r#""({VAR})""#)).unwrap());
static CONST_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r#"\bconst\s+([A-Z_][A-Z0-9_]*)\s*:\s*&(?:'static\s+)?str\s*=\s*"({VAR})""#
    ))
    .unwrap()
});
static DIRECT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r#"\benv::var(?:_os)?\(\s*"({VAR})""#)).unwrap());
static DIRECT_CONST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\benv::var(?:_os)?\(\s*([A-Z_][A-Z0-9_]*)\s*\)").unwrap());
static HELPER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r#"\b([A-Za-z_][A-Za-z0-9_]*)\(\s*"({VAR})""#)).unwrap());
static HELPER_CONST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\(\s*([A-Z_][A-Z0-9_]*)\s*[,)]").unwrap()
});
static ANY_ENV_CALL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\benv::var(?:_os)?\(").unwrap());
static CFG_ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"#\[cfg\(([^\]]*)\)\]").unwrap());
static TEST_WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\btest\b").unwrap());

/// A read of `var` in `file` (repo-relative, `/`-separated).
pub type Site = (String, String);

/// The repository root: this crate is `crates/app`.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

pub fn scan(root: &Path) -> BTreeSet<Site> {
    let mut sites = BTreeSet::new();
    for path in source_files(root) {
        let src =
            std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let rel = path
            .strip_prefix(root)
            .unwrap()
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        for var in sites_in(&strip_test_items(&src), rel.starts_with(CENTRAL)) {
            sites.insert((var, rel.clone()));
        }
    }
    sites
}

fn is_env_helper(ident: &str) -> bool {
    let lower = ident.to_ascii_lowercase();
    (lower.contains("env") || lower.contains("var"))
        && !matches!(ident, "var" | "var_os" | "set_var" | "remove_var")
}

fn skip_file_name(name: &str) -> bool {
    name.ends_with("_tests.rs") || name.ends_with("_test.rs") || name == "tests.rs"
}

fn source_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk(&root.join("crates"), &root.join("crates"), &mut out);
    out.sort();
    out
}

fn walk(dir: &Path, crates: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let rel = path.strip_prefix(crates).unwrap();
        if path.is_dir() {
            if rel
                .components()
                .next()
                .is_some_and(|c| c.as_os_str() == "workspace-hack")
            {
                continue;
            }
            walk(&path, crates, out);
        } else if path.extension().is_some_and(|e| e == "rs")
            && rel.components().any(|c| c.as_os_str() == "src")
            && !skip_file_name(&entry.file_name().to_string_lossy())
        {
            out.push(path);
        }
    }
}

// ── Lexing ────────────────────────────────────────────────────────────────

/// One kind per byte: `c` code, `s` string/char literal, `m` comment.
fn lex(src: &[u8]) -> Vec<u8> {
    let n = src.len();
    let mut kinds = Vec::with_capacity(n);
    let mut i = 0;
    while i < n {
        let ch = src[i];
        let nxt = src.get(i + 1).copied().unwrap_or(0);
        let (kind, end) = if ch == b'/' && nxt == b'/' {
            (b'm', line_end(src, i))
        } else if ch == b'/' && nxt == b'*' {
            (b'm', block_comment_end(src, i))
        } else if ch == b'"' || (matches!(ch, b'b' | b'r' | b'c') && nxt == b'"') {
            (b's', string_end(src, i))
        } else if (ch == b'r' || (matches!(ch, b'b' | b'c') && nxt == b'r'))
            && raw_string_start(src, i)
        {
            (b's', raw_string_end(src, i))
        } else if ch == b'\''
            && let Some(end) = char_literal_end(src, i)
        {
            (b's', end)
        } else {
            (b'c', i + 1)
        };
        kinds.extend(std::iter::repeat_n(kind, end - i));
        i = end;
    }
    kinds
}

fn line_end(src: &[u8], i: usize) -> usize {
    src[i..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(src.len(), |p| i + p)
}

fn block_comment_end(src: &[u8], i: usize) -> usize {
    let (mut depth, mut j) = (1, i + 2);
    while j < src.len() && depth > 0 {
        if src[j..].starts_with(b"/*") {
            depth += 1;
            j += 2;
        } else if src[j..].starts_with(b"*/") {
            depth -= 1;
            j += 2;
        } else {
            j += 1;
        }
    }
    j
}

fn string_end(src: &[u8], i: usize) -> usize {
    let mut j = i + src[i..].iter().position(|&b| b == b'"').unwrap() + 1;
    while j < src.len() {
        match src[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    src.len()
}

fn raw_string_start(src: &[u8], i: usize) -> bool {
    let mut j = i + if src[i] == b'r' { 1 } else { 2 };
    while j < src.len() && src[j] == b'#' {
        j += 1;
    }
    j < src.len() && src[j] == b'"'
}

fn raw_string_end(src: &[u8], i: usize) -> usize {
    let mut j = i + if src[i] == b'r' { 1 } else { 2 };
    let mut hashes = 0;
    while src[j] == b'#' {
        hashes += 1;
        j += 1;
    }
    let mut close = vec![b'"'];
    close.extend(std::iter::repeat_n(b'#', hashes));
    let start = j + 1;
    (start..=src.len().saturating_sub(close.len()))
        .find(|&k| src[k..].starts_with(&close))
        .map_or(src.len(), |k| k + close.len())
}

/// End of a char literal at `i`, or `None` for a lifetime (`'a`).
fn char_literal_end(src: &[u8], i: usize) -> Option<usize> {
    let n = src.len();
    if i + 1 >= n {
        return None;
    }
    if src[i + 1] == b'\\' {
        return src[i + 2..]
            .iter()
            .position(|&b| b == b'\'')
            .map(|p| i + 2 + p + 1);
    }
    (i + 2 < n && src[i + 2] == b'\'').then_some(i + 3)
}

/// `src` with every byte of a kind in `drop` blanked (newlines kept).
fn blank(src: &[u8], kinds: &[u8], drop: &[u8]) -> Vec<u8> {
    src.iter()
        .zip(kinds)
        .map(|(&c, k)| {
            if drop.contains(k) && c != b'\n' {
                b' '
            } else {
                c
            }
        })
        .collect()
}

// ── Test-code removal ─────────────────────────────────────────────────────

/// `src` with comments blanked and every `#[cfg(test)]` item blanked.
fn strip_test_items(src: &str) -> String {
    let bytes = src.as_bytes();
    let kinds = lex(bytes);
    let search = String::from_utf8_lossy(&blank(bytes, &kinds, b"ms")).into_owned();
    let mut scan = blank(bytes, &kinds, b"m");
    for attr in CFG_ATTR.captures_iter(&search) {
        let inner = attr.get(1).unwrap();
        if !names_test(inner.as_str()) {
            continue;
        }
        let whole = attr.get(0).unwrap();
        let end = item_end(&search, whole.end());
        for byte in &mut scan[whole.start()..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8_lossy(&scan).into_owned()
}

/// `cfg(test)`, `cfg(any(test, ..))`, but not `cfg(not(test))`.
fn names_test(inner: &str) -> bool {
    TEST_WORD
        .find_iter(inner)
        .any(|m| !inner[..m.start()].ends_with("not("))
}

/// End of the item that starts after the attribute ending at `i`.
fn item_end(search: &str, mut i: usize) -> usize {
    let bytes = search.as_bytes();
    let n = bytes.len();
    loop {
        while i < n && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if bytes[i..].starts_with(b"#[") {
            i = bracket_end(bytes, i + 1, b'[', b']');
            continue;
        }
        break;
    }
    let brace = bytes[i..].iter().position(|&b| b == b'{').map(|p| i + p);
    let semi = bytes[i..].iter().position(|&b| b == b';').map(|p| i + p);
    match (brace, semi) {
        (_, Some(s)) if brace.is_none_or(|b| s < b) => s + 1,
        (None, _) => n,
        (Some(b), _) => bracket_end(bytes, b, b'{', b'}'),
    }
}

fn bracket_end(bytes: &[u8], i: usize, open: u8, close: u8) -> usize {
    let mut depth = 0i32;
    for (j, &b) in bytes.iter().enumerate().skip(i) {
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return j + 1;
            }
        }
    }
    bytes.len()
}

// ── Sites ─────────────────────────────────────────────────────────────────

fn sites_in(text: &str, central: bool) -> BTreeSet<String> {
    if central {
        return LITERAL
            .captures_iter(text)
            .map(|c| c[1].to_string())
            .collect();
    }
    let mut found: BTreeSet<String> = DIRECT
        .captures_iter(text)
        .map(|c| c[1].to_string())
        .collect();
    found.extend(
        HELPER
            .captures_iter(text)
            .filter(|c| is_env_helper(&c[1]))
            .map(|c| c[2].to_string()),
    );
    let consts: BTreeMap<String, String> = CONST_DECL
        .captures_iter(text)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect();
    if consts.is_empty() {
        return found;
    }
    for c in DIRECT_CONST.captures_iter(text) {
        if let Some(var) = consts.get(&c[1]) {
            found.insert(var.clone());
        }
    }
    for c in HELPER_CONST.captures_iter(text) {
        if is_env_helper(&c[1])
            && let Some(var) = consts.get(&c[2])
        {
            found.insert(var.clone());
        }
    }
    if ANY_ENV_CALL.is_match(text) {
        found.extend(consts.into_values());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_items_are_blanked_and_strings_keep_their_braces() {
        let src = r#"
const GATE: &str = "OXY_GATE";
fn on() -> bool { std::env::var(GATE).is_ok() }
#[cfg(test)]
mod tests {
    #[test]
    fn t() { let s = "{{"; std::env::var("OXY_ONLY_IN_TESTS").ok(); }
}
fn after() { env_usize("OXY_HELPER", 1); } // std::env::var("OXY_IN_COMMENT")
"#;
        let found = sites_in(&strip_test_items(src), false);
        let want: BTreeSet<String> = ["OXY_GATE", "OXY_HELPER"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(found, want);
    }

    #[test]
    fn a_constant_in_a_file_that_reads_env_counts() {
        let src = r#"const A: &str = "OXY_A"; fn f(var: impl Fn(&str) -> Option<String>) { var(A); }
fn g() { std::env::var("OXY_B").ok(); }"#;
        let found = sites_in(&strip_test_items(src), false);
        assert!(found.contains("OXY_A") && found.contains("OXY_B"));
        let alone = sites_in(&strip_test_items(r#"const A: &str = "OXY_A";"#), false);
        assert!(
            alone.is_empty(),
            "a constant alone is a message, not a read"
        );
    }

    #[test]
    fn cfg_not_test_is_production_code() {
        let src = r#"#[cfg(not(test))] fn f() { std::env::var("OXY_PROD").ok(); }"#;
        assert!(sites_in(&strip_test_items(src), false).contains("OXY_PROD"));
    }
}
