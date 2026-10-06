//! Fails the build on a bare `AuditEntry::new` in request-handling code.
//!
//! An audit row written during a key-authenticated request must say so
//! (API-tokens design §3.7): `actor_type = api_key` and `metadata.token_id`.
//! [`AuditEntry::for_request`](super::AuditEntry::for_request) does that from
//! the [`RequestActor`](super::RequestActor) extractor; `AuditEntry::new` cannot,
//! because it never sees the request. The failure mode of forgetting is a row
//! that looks like the user did it by hand — which compiles, passes review and
//! is wrong only when someone asks "what did this key do?". That cannot be left
//! to attention.
//!
//! Workspace-wide rather than crate-local, like `entity`'s `typed_column_guard`:
//! handlers live in `oxy-app` and in the `api-*` surface crates, and a scan of
//! one crate would pass while a sibling regresses. It lives here, next to the
//! type it guards, so it runs in a unit-test binary that links in seconds.
//!
//! The scan reads **code only**: comments and string literals are blanked
//! first, so a commented-out call neither trips it nor props up an allowlist
//! entry, and prose that names the function is not a call site.

use std::path::{Path, PathBuf};

const BARE: &str = "AuditEntry::new(";
const REQUEST: &str = "AuditEntry::for_request(";

/// Files allowed a bare `AuditEntry::new`, relative to `crates/`, each with the
/// reason no request actor exists there. **Not a backlog to grow**: a handler
/// goes through `for_request`. An entry whose file no longer has a bare call
/// fails the build too, so the list cannot go stale.
const NO_REQUEST_ACTOR: &[(&str, &str)] = &[
    (
        "app-core/src/audit.rs",
        "defines `AuditEntry::new`; its unit tests build entries with no request",
    ),
    (
        "app/src/server/api/custom_apps_functions/write_record.rs",
        "the function host: the actor is the invocation's verified identity or the \
         app itself (a schedule, a webhook, a queued run), not the HTTP request",
    ),
    (
        "app/src/server/api/user_tokens/system_audit.rs",
        "token lifecycle rows no request wrote: a leak report (public, nothing \
         authenticated), the token sweeper on the global worker's tick, and the \
         sandbox maintenance loop ending a dead token's sandboxes",
    ),
    (
        "app/src/server/api/custom_apps_sandboxes/expiry_audit.rs",
        "a sandbox deleted by the expiry sweep on the global worker's tick: no \
         request, so the actor is the system; a person's create or delete goes \
         through `for_request`",
    ),
    (
        "app/src/server/api/oidc_exchange/reject.rs",
        "a refused OIDC exchange: nothing authenticated, so there is no user and no \
         credential to build a request actor from",
    ),
];

/// `crates/app-core` -> `crates/`.
fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/app-core has a parent")
        .to_path_buf()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if path.is_dir() {
            if name != "target" && name != "node_modules" {
                rust_sources(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Test code has no request either: integration tests (`/tests/`) and the
/// `*_tests.rs` / `tests.rs` files unit tests are split into.
fn is_test_file(rel: &str) -> bool {
    rel.contains("/tests/") || rel.ends_with("_tests.rs") || rel.ends_with("/tests.rs")
}

/// `src` with every comment and the inside of every string literal blanked,
/// then all whitespace removed — so a call wrapped across lines still reads as
/// one token run, and nothing in prose or data can look like code.
pub(super) fn code_only(src: &str) -> String {
    let b: Vec<char> = src.chars().collect();
    let mut out = String::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let next = b.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i = skip_block_comment(&b, i);
        } else if let Some(end) = raw_string_end(&b, i) {
            out.push('"');
            out.push('"');
            i = end;
        } else if c == '"' {
            out.push('"');
            out.push('"');
            i = skip_quoted(&b, i, '"');
        } else if c == '\'' {
            i = skip_char_or_lifetime(&b, i, &mut out);
        } else {
            if !c.is_whitespace() {
                out.push(c);
            }
            i += 1;
        }
    }
    out
}

/// Past a `/* … */`, nesting included.
fn skip_block_comment(b: &[char], start: usize) -> usize {
    let mut depth = 0;
    let mut i = start;
    while i < b.len() {
        if b[i] == '/' && b.get(i + 1) == Some(&'*') {
            depth += 1;
            i += 2;
        } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
            depth -= 1;
            i += 2;
            if depth == 0 {
                break;
            }
        } else {
            i += 1;
        }
    }
    i
}

/// Past the literal opened by `quote` at `start`, honouring `\` escapes.
fn skip_quoted(b: &[char], start: usize, quote: char) -> usize {
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            '\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
}

/// The index past a raw string (`r"…"`, `r#"…"#`, `br##"…"##`, `cr#"…"#`)
/// starting at `start`, or `None` when none starts there.
fn raw_string_end(b: &[char], start: usize) -> Option<usize> {
    let prev_is_ident = start > 0 && (b[start - 1].is_alphanumeric() || b[start - 1] == '_');
    if prev_is_ident {
        return None;
    }
    let mut i = start;
    if matches!(b.get(i), Some('b' | 'c')) {
        i += 1;
    }
    if b.get(i) != Some(&'r') {
        return None;
    }
    i += 1;
    let hashes = b[i..].iter().take_while(|c| **c == '#').count();
    i += hashes;
    if b.get(i) != Some(&'"') {
        return None;
    }
    i += 1;
    while i < b.len() {
        if b[i] == '"'
            && b[i + 1..]
                .iter()
                .take(hashes)
                .filter(|c| **c == '#')
                .count()
                == hashes
        {
            return Some(i + 1 + hashes);
        }
        i += 1;
    }
    Some(b.len())
}

/// A `'` opens a char literal (`'x'`, `'\n'`) or a lifetime (`'a`). Blank the
/// first, keep going past the second.
fn skip_char_or_lifetime(b: &[char], start: usize, out: &mut String) -> usize {
    let is_char = matches!(
        (b.get(start + 1), b.get(start + 2)),
        (Some('\\'), _) | (Some(_), Some('\''))
    );
    if is_char {
        out.push_str("''");
        skip_quoted(b, start, '\'')
    } else {
        out.push('\'');
        start + 1
    }
}

struct Scan {
    /// Non-test, non-allowlisted files with a bare call.
    offenders: Vec<String>,
    /// Allowlisted files that really do contain one.
    allowlisted_with_call: Vec<String>,
    /// How many `for_request` call sites exist, and in how many files.
    request_calls: usize,
    request_files: usize,
}

fn scan() -> Scan {
    let root = crates_root();
    let mut files = Vec::new();
    rust_sources(&root, &mut files);
    let mut s = Scan {
        offenders: Vec::new(),
        allowlisted_with_call: Vec::new(),
        request_calls: 0,
        request_files: 0,
    };
    for path in files {
        let rel = relative(&root, &path);
        if is_test_file(&rel) {
            continue;
        }
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        let code = code_only(&src);
        let calls = code.matches(REQUEST).count();
        if calls > 0 {
            s.request_calls += calls;
            s.request_files += 1;
        }
        if !code.contains(BARE) {
            continue;
        }
        if NO_REQUEST_ACTOR.iter().any(|(allowed, _)| *allowed == rel) {
            s.allowlisted_with_call.push(rel);
        } else {
            s.offenders.push(rel);
        }
    }
    s.offenders.sort();
    s
}

#[test]
fn request_handlers_build_audit_entries_with_for_request() {
    let s = scan();
    assert!(
        s.offenders.is_empty(),
        "these files call `AuditEntry::new` directly: {:?}\n\
         In a request handler, take the `RequestActor` extractor and build the entry \
         with `AuditEntry::for_request(&actor, ..)` — that is what records the API \
         key or token the request used (actor_type = api_key, metadata.token_id). \
         A bare `new` writes a row that reads as if the user acted by hand.\n\
         If there is genuinely no request (a system loop, a queued job), add the file \
         to NO_REQUEST_ACTOR in crates/app-core/src/audit/call_sites_guard.rs with the reason.",
        s.offenders
    );
}

#[test]
fn every_allowlisted_file_still_needs_its_exemption() {
    let s = scan();
    let stale: Vec<&str> = NO_REQUEST_ACTOR
        .iter()
        .map(|(file, _)| *file)
        .filter(|file| !s.allowlisted_with_call.iter().any(|f| f == file))
        .collect();
    assert!(
        stale.is_empty(),
        "allowlisted in NO_REQUEST_ACTOR but no longer calling `AuditEntry::new` in code \
         (a comment or a string does not count): {stale:?}. Remove the entry."
    );
    for (file, reason) in NO_REQUEST_ACTOR {
        assert!(
            !reason.trim().is_empty(),
            "{file}: an exemption needs a reason"
        );
    }
}

/// An empty walk would make the guard pass forever.
#[test]
fn the_scan_sees_the_handlers() {
    let s = scan();
    assert!(
        s.request_calls >= 40 && s.request_files >= 20,
        "found {} `for_request` call sites in {} files; the walk probably broke \
         rather than the handlers disappearing",
        s.request_calls,
        s.request_files
    );
}

#[test]
fn a_bare_call_in_code_is_seen_however_it_is_wrapped() {
    assert!(code_only("let e = AuditEntry::new(actor, \"x\");").contains(BARE));
    assert!(code_only("audit::AuditEntry::new (\n    a,\n)").contains(BARE));
    assert!(code_only("AuditEntry::\n    new(a)").contains(BARE));
}

#[test]
fn comments_and_strings_are_not_code() {
    for src in [
        "// AuditEntry::new(actor, \"x\")",
        "/// call `AuditEntry::new(..)` only without a request",
        "/* AuditEntry::new(a) /* nested */ AuditEntry::new(b) */",
        "let s = \"AuditEntry::new(\";",
        "let s = \"a \\\" AuditEntry::new(\";",
        "let s = r#\"AuditEntry::new(\"quoted\")\"#;",
        "let s = br\"AuditEntry::new(\";",
    ] {
        assert!(!code_only(src).contains(BARE), "{src}");
        assert!(!code_only(src).contains(REQUEST), "{src}");
    }
}

#[test]
fn code_after_a_comment_string_char_or_lifetime_is_still_seen() {
    for src in [
        "// note\nAuditEntry::new(a)",
        "/* note */ AuditEntry::new(a)",
        "let s = \"x\"; AuditEntry::new(a)",
        "let c = '\"'; AuditEntry::new(a)",
        "let c = '\\''; AuditEntry::new(a)",
        "fn f<'a>(x: &'a str) { AuditEntry::new(x) }",
        "let s = r#\"x\"#; AuditEntry::new(a)",
        "let s = cr#\"a\"b\"#; AuditEntry::new(a); let t = \"y\";",
        "let s = br#\"a\"b\"#; AuditEntry::new(a); let t = \"y\";",
    ] {
        assert!(code_only(src).contains(BARE), "{src}");
    }
}

#[test]
fn test_files_are_recognised() {
    assert!(is_test_file("app/tests/authz/audit_append_only.rs"));
    assert!(is_test_file(
        "app/src/server/api/api_keys/lifecycle_tests.rs"
    ));
    assert!(!is_test_file("app/src/server/api/api_keys/lifecycle.rs"));
    assert!(!is_test_file("app/src/server/api/frontline_admin.rs"));
}
