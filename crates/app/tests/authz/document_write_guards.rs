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
//! Anything mounted under `/api/orgs/{org_id}/…` in this module. The `org_id`
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

/// The router file that mounts this module's routes.
const ROUTER: &str = "src/server/router/global.rs";

/// Every documents handler mounted inside `build_org_routes`.
///
/// The org prefix is applied once, by `.nest("/orgs/{org_id}", …)`, so the
/// route strings inside that function are relative and carry no `org_id` to
/// match on. The function boundary IS the org scope — which is also why the
/// read routes, mounted outside it, are excluded without needing to be listed,
/// and why `favorite`/`unfavorite` are: they are personal acts, deliberately
/// not org-scoped.
///
/// # Two ways this scan went blind, both fixed
///
/// It matched a token only if it began `documents::`. The merge from main
/// brought in mounts written fully qualified —
/// `crate::server::api::operating_graph::locations::list_locations` — which is
/// one token starting with `crate::`, so a document write in that style would
/// have been invisible while the count floors stayed satisfied by the others.
/// Matched on the SUFFIX now.
///
/// And the body was taken as "up to the next top-level `fn`", while
/// `build_org_routes` is the last `fn` in the file — so the body ran to EOF and
/// the day something is appended after it, that function's mounts would be
/// scanned as org-scoped. Brace-matched now, which is what "this function's
/// body" actually means.
fn org_routes_body(src: &str) -> &str {
    let start = src
        .find("fn build_org_routes(")
        .expect("`build_org_routes` moved or was renamed — this scan is anchored on it");
    let open = start
        + src[start..]
            .find('{')
            .expect("`build_org_routes` has no body");
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
    panic!("`build_org_routes`'s body is unbalanced — the scan cannot bound it");
}

fn org_scoped_document_mounts(src: &str) -> Vec<(String, String)> {
    let body = org_routes_body(src);
    let mut out = vec![];
    for line in body.lines() {
        let t = line.trim();
        for tok in t.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':')) {
            // Suffix, not prefix: `documents::manage::create_folder` and
            // `crate::server::api::documents::manage::create_folder` are the
            // same mount written two ways, and both are house style here.
            let parts: Vec<&str> = tok.split("::").collect();
            if parts.len() >= 3 && parts[parts.len() - 3] == "documents" {
                out.push((t.to_string(), parts[parts.len() - 3..].join("::")));
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
    for module in [
        "manage",
        "categories",
        "review",
        "versions",
        "shelf",
        "handlers",
    ] {
        sources.insert(
            module,
            fs::read_to_string(format!("src/server/api/documents/{module}.rs"))
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

    // The scan can only see a handler written `…documents::<module>::<fn>`, so
    // the guarantee it needs is not a count — it is that no OTHER spelling can
    // reach the router.
    //
    // Two earlier attempts at this were unfalsifiable and both shipped. The
    // first counted `documents::` occurrences and compared them to the mounts:
    // both sides came from the same token, so a mount without it contributed
    // zero to each and the equality held. The second compared document route
    // STRINGS to mounts — 12 against 16, because a route like
    // `.patch(x).delete(y)` is one string and two handlers, so it carried four
    // slack and could never fire. A guard against blindness cannot be spelled
    // in the thing it is guarding, and it cannot be a count of two different
    // things.
    //
    // This is structural instead: an import that brings a documents submodule
    // into scope under any other name is what would let `manage::create_folder`
    // or `doc_manage::create_folder` appear in the router. There is exactly one
    // legitimate import, and anything else fails here with instructions.
    for line in router.lines() {
        let t = line.trim();
        if !t.starts_with("use ") || !t.contains("documents") {
            continue;
        }
        assert_eq!(
            t, "use crate::server::api::documents;",
            "`global.rs` imports a documents submodule under another name.\n  {t}\n\n\
             Every documents handler must be mounted as `documents::<module>::<fn>` (or its \
             fully-qualified form), because that is the only shape this scan can resolve — \
             a handler behind an alias is invisible to it and would ship unguarded. Mount it \
             through `documents::`, or teach `org_scoped_document_mounts` the new name."
        );
    }
}
