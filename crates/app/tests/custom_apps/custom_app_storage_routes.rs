//! Boundary test: the storage admin UI's URLs must match the routes oxy mounts.
//!
//! ## Why this test exists
//!
//! The Storage tab shipped calling `/api/admin/apps/storage`, while the handlers
//! were mounted inside the `/customer-apps` nest — so every call 404'd into the
//! SPA fallback, or worse, `GET /api/admin/apps/storage` fell into `/apps/{id}`
//! and tried to parse `"storage"` as a UUID. Nothing objected: the Rust tests
//! never load the TypeScript, the TypeScript tests never load the router, and
//! both halves compiled and passed.
//!
//! That is the whole failure mode — a **cross-language** contract with no
//! compiler on either side. The two files agree only because someone remembered,
//! and a reviewer who doesn't hold the router's nesting in their head can't catch
//! it. So the objection has to be mechanical.
//!
//! ## What it checks
//!
//! Every URL literal in `services/api/customAppStorage.ts`, normalized back into
//! an axum route pattern, must appear as a mounted route in `router/global.rs`.
//! It deliberately does NOT try to parse either language properly — a regex over
//! source is enough to catch a path that moved, and cheap enough to never rot.
//!
//! Related in spirit: `authz_boundaries.rs`, `custom_apps_boundary.rs`.

use crate::common::read_repo_file;

/// The nest the storage handlers are mounted under (`router/global.rs`). The
/// frontend's `apiClient` has `/api` as its baseURL, so a TS path of
/// `/customer-apps/storage` is `/api/customer-apps/storage` on the wire.
const NEST: &str = "/customer-apps";

/// Pull the quoted/backticked URL literals out of the TS service.
fn frontend_paths(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (open, close) in [('"', '"'), ('`', '`')] {
        let mut rest = src;
        while let Some(start) = rest.find(open) {
            let after = &rest[start + 1..];
            let Some(end) = after.find(close) else { break };
            let literal = &after[..end];
            if literal.starts_with('/') {
                out.push(literal.to_string());
            }
            rest = &after[end + 1..];
        }
    }
    out
}

/// Members of the exported service object — one per endpoint the UI calls.
///
/// Matches a two-space-indented `name:` line, which is what the object literal's
/// top level looks like after Biome formats it. Deliberately NOT
/// `matches("apiClient.")`: Biome breaks the call across lines, so that appears
/// once in the whole file regardless of how many methods there are.
fn ts_service_methods(src: &str) -> usize {
    src.lines()
        .filter(|l| l.starts_with("  ") && !l.starts_with("   "))
        .filter_map(|l| l.trim_start().split_once(':'))
        .filter(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric()))
        .count()
}

/// `/customer-apps/${appId}/storage/objects` -> `/{id}/storage/objects`, i.e.
/// the string that should literally appear in a `.route(...)` call.
fn to_route_pattern(path: &str) -> Option<String> {
    let relative = path.strip_prefix(NEST)?;
    let mut out = String::new();
    let mut rest = relative;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}')?;
        // Every interpolation in this service is a path id; the route names it
        // `{id}` (app) or `{org_id}` (org).
        let name = after[..end].trim();
        out.push_str(if name.to_ascii_lowercase().contains("org") {
            "{org_id}"
        } else {
            "{id}"
        });
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

#[test]
fn storage_ui_paths_match_mounted_routes() {
    let service = read_repo_file("web-app/src/services/api/customAppStorage.ts");
    let router = read_repo_file("crates/app/src/server/router/global.rs");

    let paths = frontend_paths(&service);
    // A FLOOR, not merely non-empty. `frontend_paths` pairs backticks
    // sequentially across the whole file, comments included, so the two
    // template-literal paths pair correctly only while the backticks preceding
    // them are even in number.
    //
    // A balanced `ident` in a comment is safe — it adds two. An UNBALANCED one
    // is not: a single stray backtick before them shifts every later pair, both
    // template literals drop out, the three double-quoted paths survive, and a
    // `!is_empty()` check still passes while the test covers 3 of 5 endpoints.
    // Verified by inserting one: the assertion below reports `found 3`.
    //
    // The `"` pass has the identical hazard — it is the same sequential pairing —
    // it just has no unbalanced quotes to trip over today. The floor below covers
    // both; only this prose was ever backtick-specific.
    //
    // That silent narrowing is exactly the failure this file's header warns
    // about, and the sibling handler test already carries the same guard.
    // Derived, not a literal. A hardcoded `5` passes at 5-of-6 the day a sixth
    // endpoint lands, which is precisely how the handler guard below shipped one
    // behind. One service method calls one endpoint, so the method count IS the
    // expected path count.
    let methods = ts_service_methods(&service);
    assert!(
        paths.len() >= methods,
        "customAppStorage.ts declares {methods} service methods but only {} URL \
         literal(s) were extracted: {paths:?} — an unbalanced backtick or quote in a \
         comment shifts the pairing and silently drops paths",
        paths.len()
    );

    let mut failures = Vec::new();
    for path in &paths {
        let Some(pattern) = to_route_pattern(path) else {
            failures.push(format!(
                "{path} is not under the `{NEST}` nest the storage handlers are mounted in \
                 (router/global.rs). Either move the route or fix the path."
            ));
            continue;
        };
        // Match the route literal as it appears in a `.route("…"` call.
        if !router.contains(&format!("\"{pattern}\"")) {
            failures.push(format!(
                "{path} normalizes to route `{pattern}`, which is not mounted in \
                 router/global.rs. The UI would 404 into the SPA fallback."
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "storage UI calls routes that are not mounted:\n  {}",
        failures.join("\n  ")
    );
}

#[test]
fn every_storage_handler_is_reachable() {
    // The mirror of the test above: a handler nobody routes is dead code that
    // reads as shipped. Catches the opposite drift — deleting a route but
    // leaving the handler (and its doc comment claiming a URL) behind.
    //
    // The list is DERIVED from the source, not maintained by hand. A hardcoded
    // one silently stopped covering `history`, the first handler added after this
    // test shipped — a list you have to remember to update is a list that goes
    // stale exactly when it matters.
    let router = read_repo_file("crates/app/src/server/router/global.rs");
    let handlers = read_repo_file("crates/app/src/server/api/admin/apps/storage.rs");

    // A `pub async fn` here is a route handler *or* a testable query helper
    // (`fleet_rows_scoped`), and only the first kind belongs in the router. The
    // discriminator is the return type: handlers answer HTTP. Splitting on the
    // name instead — an allowlist, a `_scoped` suffix — is the kind of
    // convention that holds until someone names a handler differently, and then
    // this test quietly stops covering it.
    //
    // **The default is HANDLER**, and the predicate below is therefore a
    // negative match. An earlier version listed the handler shapes positively
    // (`Response`, `Json<`) and let everything else fall into `helpers`, which
    // made the default *helper* — so `-> StatusCode`, `-> Redirect`,
    // `-> Html<String>` and `-> (StatusCode, String)`, all of which axum accepts,
    // would have been dropped from the check entirely. That is precisely the
    // failure this test exists to prevent, reachable by return-type spelling.
    // Inverted, a new query helper fails loudly until it declares a shape this
    // recognizes, and no handler can slip out.
    let lines: Vec<&str> = handlers.lines().collect();
    let mut names = Vec::new();
    let mut helpers = Vec::new();
    let mut declared = 0usize;
    for (i, line) in lines.iter().enumerate() {
        let Some(rest) = line.strip_prefix("pub async fn ") else {
            continue;
        };
        declared += 1;
        let name = rest
            .split(['(', '<'])
            .next()
            .map(str::trim)
            .expect("fn name");
        // Walk to the `-> …` that ends the signature; `fn foo(` may wrap over
        // several lines, so this cannot just look at the declaring line.
        //
        // The walk stops at the line that opens the BODY. Without that bound it
        // is only accidentally correct — a unit-returning `pub async fn` has no
        // arrow at all, so the scan would run on into the body and classify on
        // whatever arrow it met first (a closure's explicit return type, a
        // `where` clause). Under a default of "handler" that misreads as a
        // helper the moment such an arrow names `DbErr`, which is the one
        // direction this test cannot afford.
        let mut ret: Option<&str> = None;
        for line in lines[i..].iter().take(12) {
            // `-> ` first: the body-opening line is usually `) -> Ret {`, which
            // matches both checks.
            if let Some((_, r)) = line.split_once("-> ") {
                ret = Some(r.trim_end_matches(" {").trim());
                break;
            }
            if line.contains('{') {
                break; // body opened, no arrow: returns `()`
            }
        }
        // The one recognized non-HTTP shape: a query helper hands the caller a
        // `DbErr` to deal with, which no handler can do — axum has nothing to
        // turn it into. Add shapes here as helpers grow, deliberately and one at
        // a time; anything unrecognized — including a unit return — stays a
        // handler and gets checked.
        if ret.is_some_and(|r| r.contains("DbErr")) {
            helpers.push(name);
        } else {
            names.push(name);
        }
    }
    // Completeness, not a literal floor: every `pub async fn` must land in one
    // bucket. A hardcoded minimum both goes stale (it said 5 against six
    // handlers) and cannot notice a signature reformat that drops one.
    assert!(declared > 0, "no handlers found — did storage.rs move?");
    // Guards a vacuous pass, NOT the old "did the positive match miss one"
    // worry — the inversion retired that direction. If every fn in the file
    // reads as a helper, the loop below checks nothing and reports success; the
    // realistic way to get there is handlers moving to another module while
    // this test keeps pointing here.
    assert!(
        !names.is_empty(),
        "no handlers left in storage.rs — every `pub async fn` classified as a query \
         helper ({helpers:?}), so the mount check below would pass without checking \
         anything. Did the handlers move?"
    );
    assert_eq!(
        names.len() + helpers.len(),
        declared,
        "classified {} of {declared} `pub async fn` lines — an unclassified one is a \
         handler this test would not notice being unmounted",
        names.len() + helpers.len()
    );

    for name in names {
        assert!(
            router.contains(&format!("storage::{name}")),
            "`storage::{name}` is not mounted in router/global.rs.\n\
             If it IS a route handler, mount it there. If it is a query helper, this \
             test misread it: helpers are recognized by returning a `DbErr` (see the \
             note above the classifier), so give it that shape or extend the predicate \
             deliberately. Do NOT mount a helper to silence this."
        );
    }
}

/// Marker a line can carry to say "this URL is quoted on purpose".
///
/// The design doc's traps section has to be able to describe the original bug,
/// and a check that bans writing down what went wrong is worse than no check.
const HISTORICAL: &str = "(historical)";

/// Does this line claim an `admin/apps/.../storage` **URL**, as opposed to merely
/// mentioning both words?
///
/// Order- and shape-sensitive on purpose: the earlier version matched
/// `admin/apps/` and `storage` anywhere on the line, which would have failed the
/// build on the sentence "the frontend called `/admin/apps/...`" the moment
/// someone added the word storage to it.
fn claims_an_admin_storage_url(line: &str) -> bool {
    // Whole-line, deliberately — and worth knowing it is the one asymmetry left
    // here. A line carrying the marker is exempt in full, so a deliberate
    // historical quote and an accidental live claim in the SAME line (a
    // two-column markdown row, say) would both pass. Making the exemption
    // per-occurrence means pairing each marker to a specific URL, which is more
    // syntax than the problem is worth; the contrivance needed to hit it is a
    // fair trade for a rule anyone can apply by eye.
    if line.contains(HISTORICAL) {
        return false;
    }
    // EVERY occurrence, not just the first. `line.find` returns one index, so a
    // line opening with bare prose would mask a real claim later on the same line:
    // "the handlers left `/admin/apps/` — they are not at `/admin/apps/storage`
    // any more" has an empty first token and would read as clean. That is the
    // exact mirror of the false positive this predicate was rewritten to fix, and
    // the traps section is precisely where such a sentence gets written.
    line.match_indices("admin/apps/").any(|(idx, m)| {
        // Only the rest of THAT path token counts — `storage` elsewhere in the
        // sentence is prose, not a claim about where the route lives.
        let rest = &line[idx + m.len()..];
        rest.chars()
            .take_while(|c| !c.is_whitespace() && !matches!(c, '`' | ')' | ',' | '"'))
            .collect::<String>()
            .contains("storage")
    })
}

#[test]
fn prose_states_the_real_url() {
    // The original bug was visible in the source the whole time: the doc comments
    // said `/api/admin/apps/storage` while the mount said `/customer-apps`. If the
    // prose and the mount disagree, the prose is what a reader trusts.
    //
    // Scans every file that has carried this URL, because it survived being fixed
    // in Rust (round 1), then in one markdown file (round 3), then in a second
    // one — each time because the check was scoped to where it had last been
    // found.
    let sources = [
        "crates/app/src/server/api/admin/apps/storage.rs",
        "internal-docs/customer-apps-functions.md",
        "internal-docs/2026-08-05-custom-app-asset-lifecycle-design.md",
    ];

    let mut bad = Vec::new();
    for rel in sources {
        let text = read_repo_file(rel);
        for line in text.lines() {
            if claims_an_admin_storage_url(line) {
                bad.push(format!("{rel}: {}", line.trim()));
            }
        }
    }

    assert!(
        bad.is_empty(),
        "these lines claim an `admin/apps/...storage` URL, but the handlers are mounted \
         under `/api{NEST}/...`. If the old path is being quoted deliberately, add \
         `{HISTORICAL}` to the line:\n  {}",
        bad.join("\n  ")
    );
}

// No `#[cfg(test)]`: an integration-test file is compiled with `--test`, which
// implies `--cfg test`, so the gate could never exclude anything — and it read
// as if this module were conditional while the `#[test]` fns above are not.
mod predicate_tests {
    use super::{HISTORICAL, claims_an_admin_storage_url};

    #[test]
    fn catches_a_real_claim_in_either_prefix_form() {
        assert!(claims_an_admin_storage_url(
            "/// `GET /api/admin/apps/storage` — fleet"
        ));
        // Prose routinely drops the `/api`, which is how the third copy survived.
        assert!(claims_an_admin_storage_url(
            "| `GET /admin/apps/storage/meter/{org}` |"
        ));
    }

    #[test]
    fn ignores_prose_that_merely_mentions_both_words() {
        // The design doc's traps section says exactly this.
        assert!(!claims_an_admin_storage_url(
            "into the `/customer-apps` nest; the frontend called `/admin/apps/...`. Both"
        ));
        assert!(!claims_an_admin_storage_url(
            "the storage handlers moved out of `/admin/apps/` entirely"
        ));
    }

    #[test]
    fn a_claim_later_on_the_line_is_not_masked_by_earlier_prose() {
        // The first `admin/apps/` here is bare, so a `find`-based predicate stops
        // at an empty token and calls the line clean.
        assert!(claims_an_admin_storage_url(
            "the handlers left `/admin/apps/` — they are not at `/admin/apps/storage` any more"
        ));
    }

    #[test]
    fn honours_the_opt_out_marker() {
        let line = format!("once served at `/admin/apps/storage` {HISTORICAL}");
        assert!(!claims_an_admin_storage_url(&line));
    }

    #[test]
    fn leaves_the_legitimate_admin_routes_alone() {
        assert!(!claims_an_admin_storage_url(
            "`GET /api/admin/apps/{id}/functions` lists the app's functions"
        ));
    }
}
