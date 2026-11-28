//! The authorization caller (`contracts_authz::Caller`) is built, and its
//! per-request grant memo written, only where this file says.
//!
//! A `Caller` is who every authorization read believes is asking. Its parts
//! carry the token's reach, the service account's standing and a configured
//! service's platform standing, and its grant memo holds the platform grant the
//! request already read. So a caller built from invented parts, or a memo
//! written with an invented grant, holds whatever standing it claims, and
//! nothing would fail to compile. Both doors are public only because their one
//! honest writer is another crate:
//!
//! - `Caller::from_parts` is called with what authentication projected
//!   (`oxy_auth::caller_parts`), never with hand-built parts;
//! - the grant memo (`Caller::grant_memo`, `GrantMemo`) is reached only by the
//!   platform-standing read in `oxy_server_authz::globals` and its tests.
//!
//! This guard pins the exact set of files on each door, so a new caller fails
//! the build and has to be added here, where a reviewer sees it (C3a,
//! 2026-11-28; the widening C2 flagged in `internal-docs/crate-tiers.md`).
//! C3b and C4b add the custom-app and workspace-runtime sites that
//! authenticate a request themselves.
//!
//! It reads text, so it pins what a reviewer would grep for: a call through a
//! renamed import (`use contracts_authz::Caller as C; C::from_parts(..)`)
//! escapes it, and is the kind of thing review is for.

use std::path::{Path, PathBuf};

/// One door, and the files allowed through it.
struct Door {
    /// Literal substring, matched on non-comment lines.
    needle: &'static str,
    /// What the needle stands for, for the failure message.
    what: &'static str,
    /// The files (as `../<crate>/…`) that may contain it. Sorted.
    allowed: &'static [&'static str],
}

const DOORS: &[Door] = &[
    Door {
        needle: "Caller::from_parts(",
        what: "builds an authorization caller from parts",
        allowed: &[
            // The custom-app data-plane gate (`gates`): it authenticates the
            // request itself, so it builds the request's caller from
            // `oxy_auth::caller_parts` with the surface's enforcer (C3b-2).
            "../custom-apps/src/gate.rs",
            // The custom-app serve resolver (`resolve`), for the same reason:
            // the bundle path and every app-keyed surface authenticate here.
            "../custom-apps/src/resolve.rs",
            // `oxy_server_authz::Caller::of`: the request's caller, from
            // `oxy_auth::caller_parts`.
            "../server-authz/src/caller.rs",
        ],
    },
    Door {
        needle: "CallerParts",
        what: "names the parts a caller is built from",
        allowed: &[
            // The projection that fills them in.
            "../auth/src/caller_parts.rs",
            // Their definition and its re-export.
            "../authz/src/caller_parts.rs",
            "../authz/src/lib.rs",
            // `from_parts` and `without_credential`, and the C2 re-export.
            "../contracts-authz/src/caller.rs",
            "../contracts-authz/src/lib.rs",
        ],
    },
    Door {
        needle: "grant_memo()",
        what: "reaches a caller's per-request platform grant memo",
        allowed: &[
            // A wrapped port caller keeps the memo a guard filled.
            "../server-authz/src/enforcement.rs",
            // The one reader and writer: the uncached platform-standing read.
            "../server-authz/src/globals.rs",
            "../server-authz/src/globals_sandbox_tests.rs",
        ],
    },
    Door {
        needle: "GrantMemo",
        what: "names the grant memo type",
        allowed: &[
            "../contracts-authz/src/caller.rs",
            "../contracts-authz/src/grant_memo.rs",
            "../contracts-authz/src/lib.rs",
        ],
    },
];

/// `Caller::from_parts` outside the port crate is called with the projection:
/// this must appear in the call's argument, which rustfmt may put on the next
/// line.
const PROJECTION_CALL: &str = "caller_parts(";

/// Does `line`, a non-comment line, contain `needle` other than in its own
/// definition (`fn name(`)?
fn mentions(line: &str, needle: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") {
        return false;
    }
    line.match_indices(needle)
        .any(|(at, _)| !line[..at].ends_with("fn "))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if name == "target" || name.starts_with('.') {
                continue;
            }
            rust_files(&path, out);
        } else if name.ends_with(".rs") {
            out.push(path);
        }
    }
}

fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root.join("..")).unwrap_or(path);
    format!("../{}", rel.display()).replace('\\', "/")
}

/// Every `.rs` file under `crates/`, `src` and `tests` alike, but this crate's.
fn scanned_files() -> Vec<(String, String)> {
    let root = crate::guards_root();
    let mut files = Vec::new();
    rust_files(&root.join(".."), &mut files);
    assert!(
        files.len() > 100,
        "walked only {} files under {}: the scan would pass by finding nothing",
        files.len(),
        root.join("..").display()
    );
    let guards = root.canonicalize().expect("crates/guards");
    files
        .into_iter()
        .filter(|path| {
            !path
                .canonicalize()
                .map(|p| p.starts_with(&guards))
                .unwrap_or(false)
        })
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            Some((relative(&root, &path), text))
        })
        .collect()
}

#[test]
fn each_caller_door_has_exactly_its_pinned_files() {
    let files = scanned_files();
    let mut wrong = Vec::new();
    for door in DOORS {
        let mut found: Vec<&str> = files
            .iter()
            .filter(|(_, text)| text.lines().any(|line| mentions(line, door.needle)))
            .map(|(rel, _)| rel.as_str())
            .collect();
        found.sort_unstable();
        if found != door.allowed {
            wrong.push(format!(
                "  `{}` ({}):\n    pinned: {:?}\n    found:  {:?}",
                door.needle, door.what, door.allowed, found
            ));
        }
    }
    assert!(
        wrong.is_empty(),
        "\nThe files on a caller door changed:\n\n{}\n\n\
         A caller built from invented parts, or a grant memo written by hand, holds \
         whatever standing it claims. Build the request's caller with \
         `contracts_authz::Caller::from_parts(oxy_auth::caller_parts(..), authz)` (or \
         `oxy_server_authz::Caller::of`), and leave the memo to `globals`. If the new \
         site is one of those, add it to DOORS in \
         crates/guards/tests/integration/caller_has_pinned_constructors.rs with the \
         reason; if a site went away, remove it.\n",
        wrong.join("\n\n")
    );
}

/// Outside the port crate, the parts `from_parts` takes are the projection's.
#[test]
fn from_parts_takes_the_projection() {
    let needle = DOORS[0].needle;
    let mut bare = Vec::new();
    for (rel, text) in scanned_files() {
        if rel.starts_with("../contracts-authz/") {
            continue;
        }
        let lines: Vec<&str> = text.lines().collect();
        for (i, line) in lines.iter().enumerate() {
            if !mentions(line, needle) {
                continue;
            }
            let at = line.find(needle).expect("mentions found it");
            let call = std::iter::once(&line[at..])
                .chain(lines.iter().skip(i + 1).take(2).copied())
                .collect::<Vec<_>>()
                .join(" ");
            if !call.contains(PROJECTION_CALL) {
                bare.push(format!("  {rel}:{}: {}", i + 1, line.trim()));
            }
        }
    }
    assert!(
        bare.is_empty(),
        "\n`{needle}` outside contracts-authz must take `oxy_auth::caller_parts(..)`:\n\n{}\n",
        bare.join("\n")
    );
}
