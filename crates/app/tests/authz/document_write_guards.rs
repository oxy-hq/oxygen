//! Boundary test: every document WRITE stays behind `OrgAdmin`.
//!
//! ## Why this file exists
//!
//! Four comments in the documents module said, in the same words, that mounting
//! a manage handler behind a different guard "fails
//! `manage_documents_ring_matches_the_shipped_guard` rather than shipping".
//!
//! That test is real and it is good, but it does not do this. It calls
//! `allows(facts, Action::ManageDocuments, resource)` and compares the answer
//! against a hand-written closure — a check on the MODEL. It never reads a
//! router, a handler signature or an extractor, so changing `create_document`
//! from `OrgAdmin` to `OrgMember` leaves it green. `Action::ManageDocuments`
//! has no runtime call site at all; the handlers take the extractor directly,
//! which is what `CLAUDE.md` prescribes.
//!
//! So the claim named a safety net nobody had built. This is the net. It is a
//! source scan for the same reason `authz_boundaries.rs` is one: the objection
//! has to be mechanical, because a reviewer who does not already know the model
//! will not catch the swap, and the shortest path is always to take whichever
//! extractor the handler beside it took.
//!
//! ## What counts as a write
//!
//! Anything mounted under `/api/orgs/{org_id}/…` by the documents crate. The `org_id`
//! path segment IS the write surface — reads take the org as a query parameter
//! and gate on `resolve_standing` instead, which is a filter rather than a
//! ring. So the rule is stated against the route shape rather than against a
//! list of function names that would drift.
//!
//! ## The exceptions are named, and are the interesting part
//!
//! `favorite` and `unfavorite` are writes that are deliberately NOT `OrgAdmin`:
//! a bookmark is a personal act on something the caller can already read, and
//! `shelf.rs` explains why un-favoriting deliberately skips even the read gate.
//! They are mounted without the `org_id` segment precisely because they are not
//! org-scoped, so the route-shape rule already excludes them — this is written
//! down so that the next person to add one knows which side they are on.

use std::fs;

/// The documents surface lives in the `oxy-api-documents` sibling crate, which
/// `oxy-app` does not depend on — so this reads its sources by path, relative to
/// `crates/app`. A source scan needs no dependency edge.
const CRATE_SRC: &str = "../api-documents/src";

/// The file that mounts the documents routes.
const ROUTER: &str = "../api-documents/src/router.rs";

/// The documents crate's handler modules — a mount token is `<module>::<fn>`
/// with `<module>` one of these.
const MODULES: &[&str] = &[
    "ask",
    "ask_sessions",
    "categories",
    "handlers",
    "manage",
    "review",
    "search",
    "shelf",
    "versions",
];

/// Every documents handler mounted inside `org_routes`.
///
/// The org prefix is applied once, by `.nest("/orgs/{org_id}", org_routes())`,
/// so the route strings inside that function are relative and carry no
/// `org_id` to match on. The function boundary IS the org scope — which is also
/// why the read routes, mounted in `read_routes`, are excluded without needing
/// to be listed, and why `favorite`/`unfavorite` are: they are personal acts,
/// deliberately not org-scoped.
///
/// # Two ways this scan went blind, both fixed
///
/// It matched a token only if it began with the module path, so a mount written
/// fully qualified (`crate::manage::create_folder`) would have been invisible
/// while the count floors stayed satisfied by the others. Matched on the
/// SUFFIX `<module>::<fn>` now.
///
/// And the body was taken as "up to the next top-level `fn`", so the day
/// something is appended after the org builder, its mounts would be scanned as
/// org-scoped. Brace-matched now, which is what "this function's body"
/// actually means.
fn org_routes_body(src: &str) -> &str {
    let start = src
        .find("fn org_routes(")
        .expect("`org_routes` moved or was renamed — this scan is anchored on it");
    let open = start + src[start..].find('{').expect("`org_routes` has no body");
    let mut depth = 0usize;
    for (i, c) in src[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &src[open..open + i];
                }
            }
            _ => {}
        }
    }
    panic!("`org_routes`'s body is unbalanced — the scan cannot bound it");
}

fn org_scoped_document_mounts(src: &str) -> Vec<(String, String)> {
    let body = org_routes_body(src);
    let mut out = vec![];
    for line in body.lines() {
        let t = line.trim();
        for tok in t.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')) {
            // Suffix, not prefix: `manage::create_folder` and
            // `crate::manage::create_folder` are the same mount written two
            // ways.
            let parts: Vec<&str> = tok.split("::").collect();
            if parts.len() >= 2 && MODULES.contains(&parts[parts.len() - 2]) {
                out.push((t.to_string(), parts[parts.len() - 2..].join("::")));
            }
        }
    }
    out
}

/// Read a handler's parameter list — from `pub async fn NAME(` to the closing
/// `)` of the signature.
fn signature_of(src: &str, name: &str) -> Option<String> {
    let start = src.find(&format!("pub async fn {name}("))?;
    let rest = &src[start..];
    let end = rest.find(") -> ")?;
    Some(rest[..end].to_string())
}

#[test]
fn every_org_scoped_document_write_takes_the_orgadmin_extractor() {
    let router = fs::read_to_string(ROUTER).expect("read the router");
    let mounts = org_scoped_document_mounts(&router);

    assert!(
        mounts.len() >= 12,
        "found only {} org-scoped document mounts — the scan stopped matching the router, \
         which makes this test vacuous rather than passing",
        mounts.len()
    );

    let mut sources = std::collections::HashMap::new();
    for module in MODULES {
        sources.insert(
            *module,
            fs::read_to_string(format!("{CRATE_SRC}/{module}.rs"))
                .unwrap_or_else(|e| panic!("read {module}.rs: {e}")),
        );
    }

    let mut checked = 0;
    for (line, handler) in &mounts {
        let mut parts = handler.rsplit("::");
        let func = parts.next().expect("a function name");
        let module = parts.next().expect("a module name");
        let Some(src) = sources.get(module) else {
            continue;
        };
        let Some(sig) = signature_of(src, func) else {
            continue;
        };
        assert!(
            sig.contains("OrgAdmin("),
            "`{module}::{func}` is mounted org-scoped but does not take the `OrgAdmin` \
             extractor.\n  route: {line}\n  signature: {sig}\n\n\
             Every write under /api/orgs/{{org_id}}/ goes through `Ring::OrgAdmin`. If this \
             handler is genuinely a personal act rather than an org one — as `favorite` is — \
             mount it without the org segment and say so in this file's exception list.",
        );
        checked += 1;
    }

    assert_eq!(
        checked,
        mounts.len(),
        "resolved {checked} handler signatures out of {} mounts — a mount whose signature \
         cannot be found is one this test silently skips, which is how a scan goes blind \
         while still passing",
        mounts.len()
    );

    // The scan can only see a handler written `<module>::<fn>`, so the
    // guarantee it needs is not a count — it is that no OTHER spelling can
    // reach the router.
    //
    // Two earlier attempts at this were unfalsifiable and both shipped. The
    // first counted module-path occurrences and compared them to the mounts:
    // both sides came from the same token, so a mount without it contributed
    // zero to each and the equality held. The second compared document route
    // STRINGS to mounts — 12 against 16, because a route like
    // `.patch(x).delete(y)` is one string and two handlers, so it carried four
    // slack and could never fire. A guard against blindness cannot be spelled
    // in the thing it is guarding, and it cannot be a count of two different
    // things.
    //
    // This is structural instead: an import that brings a handler function, an
    // aliased module or a glob into the router's scope is what would let
    // `create_folder` or `doc_manage::create_folder` appear in it. The router
    // may import this crate's modules by their own names and nothing else from
    // the crate; anything else fails here with instructions.
    for stmt in crate_imports(&router) {
        let names = stmt
            .trim_start_matches("use crate::")
            .trim_end_matches(';')
            .trim_start_matches('{')
            .trim_end_matches('}');
        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            assert!(
                MODULES.contains(&name),
                "`router.rs` imports `{name}` from the documents crate.\n  {stmt}\n\n\
                 Every documents handler must be mounted as `<module>::<fn>`, because that \
                 is the only shape this scan can resolve — a handler imported by name, \
                 behind an alias or through a glob is invisible to it and would ship \
                 unguarded. Import the module, or teach `org_scoped_document_mounts` the \
                 new shape."
            );
        }
    }
}

/// Every crate-relative `use` statement in `src`, whitespace collapsed so a
/// rustfmt-wrapped import reads as one line. A `super::` import is refused
/// outright: the router sits at the crate root, so it could only mean the same
/// thing spelled a way this scan does not read.
fn crate_imports(src: &str) -> Vec<String> {
    let mut out = vec![];
    for (i, _) in src.match_indices("\nuse ") {
        let tail = &src[i + 1..];
        let end = tail.find(';').map_or(tail.len(), |j| j + 1);
        let stmt = tail[..end].split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            !stmt.starts_with("use super::") && !stmt.starts_with("use self::"),
            "`router.rs` imports relatively — {stmt}. Use `crate::` so this scan can read it."
        );
        if stmt.starts_with("use crate::") {
            out.push(
                stmt.replace("{ ", "{")
                    .replace(" }", "}")
                    .replace(",}", "}"),
            );
        }
    }
    assert!(
        !out.is_empty(),
        "`router.rs` imports nothing from the documents crate — the handlers are \
         reached some other way, which this scan cannot see"
    );
    out
}
