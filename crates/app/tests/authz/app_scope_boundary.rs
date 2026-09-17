//! Boundary test: **scope stays enforced** on the custom-app admin surface.
//!
//! ## Why a source-scanning test
//!
//! A platform grant is `(capabilities × scope)`. Capabilities are gated by a guard
//! (`platform_cap_guard`), which the type system helps with — a route either carries the
//! layer or it doesn't. Scope is different: `Resource::platform()` has no org, so no
//! guard can consult it, and enforcement is necessarily spread between one middleware
//! and a handful of handlers.
//!
//! That is precisely the shape that rots. The app console has ~20 `/{id}` routes, and a
//! twenty-first added next quarter inherits the middleware for free — but a new
//! `batch/archive` endpoint, or a new handler taking an org from its body, does not. One
//! of those forgetting is not cosmetic: `batch/delete` without a scope check lets a grant
//! bounded to one org delete apps in every other one, with no discovery step at all.
//!
//! A reviewer who doesn't already hold this invariant in their head won't catch it. So
//! the objection is mechanical: this test fails the build when a scope-bearing handler
//! stops calling its scope check.
//!
//! ## What it does NOT prove
//!
//! It reads source, not behaviour — it proves the calls are *present*, not that they are
//! correct. The behavioural half lives in `oxy-authz`'s unit tests (which pin what a
//! bounded grant may reach) and `authz_loader_differential.rs` (which proves the loader
//! reads scope from real rows). This test only stops the wiring from silently going away.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/app.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/app has a grandparent")
        .to_path_buf()
}

/// Read a source file **with comment lines removed**.
///
/// Every assertion below searches for a snippet of code. A snippet is also, very often,
/// something the surrounding comment explains by name — so a plain substring check can be
/// satisfied by the prose that documents the very call it is supposed to be guarding.
///
/// That is not hypothetical. Two assertions in this file shipped that way and had to be
/// caught by mutation testing: one matched `split_by_scope`, which the comment above each
/// call names; the next matched `Cap::OperatePlatform`, named in two doc comments. Both
/// stayed green with the real call deleted. Writing the needle more precisely fixes one
/// assertion; stripping comments here fixes the whole class, so the next assertion
/// someone adds cannot reintroduce it.
///
/// Drop whole-line comments, keep everything else.
///
/// Deliberately line-based and dumb: `//` and `///` at the start of a trimmed line. The
/// failure mode this prevents is a needle that appears ONLY in prose, and prose in these
/// files is line-comments.
///
/// **Two known gaps**, both leaving prose that a needle could match:
///
/// * a **trailing** comment — `foo(); // see Cap::OperatePlatform` — keeps its whole
///   line, comment included. This is the live one: the `Cap::OperatePlatform` assertion
///   dropped its trailing paren precisely because comment-stripping made the bare name
///   code-only, and that reasoning holds for line comments but not for trailing ones.
/// * a `/* … */` block, which is not stripped either.
///
/// Neither shape exists in the scanned files today. If one appears, this is the function
/// to extend, and [`strip_comments_removes_prose_and_keeps_code`] is where the current
/// behaviour is pinned — including, deliberately, that a trailing comment survives, so
/// the gap is a recorded fact rather than an assumption.
fn strip_comments(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    let raw =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    strip_comments(&raw)
}

/// [`strip_comments`] must actually strip, and must not eat code.
///
/// Without this, someone simplifies `read` back to a plain `read_to_string` — every
/// assertion still passes, and the whole file quietly becomes comment-satisfiable again.
/// The protection is invisible when it works, which is exactly the kind that needs a
/// test of its own.
///
/// Tested against an INLINE fixture, not a real source file. The first version of this
/// test asserted that one particular sentence in `app_publish_tokens.rs`'s module doc had
/// been stripped — so rewording that sentence, an ordinary docs edit with no reason to
/// think about this test, would have made the assertion vacuously true and re-opened the
/// whole hole. Which is the same defect this test exists to catch, one level up: a
/// protection whose own guard can be disabled by an unrelated change.
#[test]
fn strip_comments_removes_prose_and_keeps_code() {
    let stripped = strip_comments(
        "// a line comment mentioning needle_token\n\
         /// a doc comment mentioning needle_token\n\
         let x = real_code();\n\
             // indented comment mentioning needle_token\n\
         let y = other_code(); // trailing comment\n",
    );

    assert!(
        !stripped.contains("a line comment"),
        "line comments survived stripping"
    );
    assert!(
        !stripped.contains("a doc comment"),
        "doc comments survived stripping"
    );
    assert!(
        !stripped.contains("indented comment"),
        "indented comments survived stripping — leading whitespace must be trimmed first"
    );
    assert!(
        stripped.contains("let x = real_code();") && stripped.contains("let y = other_code();"),
        "stripping ate real code"
    );
    assert_eq!(
        stripped.matches("needle_token").count(),
        0,
        "a token that appears ONLY in whole-line comments must be gone — that is the \
         entire point"
    );

    // ── CHARACTERISATION, not a requirement ──────────────────────────────────────
    // Unlike every other assertion here, a GREEN result below means the known gap is
    // still open: a trailing comment survives stripping, so a needle naming something
    // mentioned after `//` on a code line remains satisfiable by that mention. It is
    // pinned so the limit is a recorded fact rather than an assumption, and so that
    // closing the gap is a deliberate act — this test will announce it.
    //
    // If it fails, `strip_comments` got better. Invert this assertion and update the
    // gap note on that function; do not delete either.
    assert!(
        stripped.contains("// trailing comment"),
        "trailing comments are now stripped — a real improvement, but the documented gap \
         in `strip_comments` and this assertion both need updating to match"
    );
}

/// [`read`] must actually APPLY the stripping.
///
/// Testing `strip_comments` against an inline fixture proves the function works; it
/// says nothing about whether `read` still calls it. Someone simplifying `read` back to
/// a bare `read_to_string` would leave the fixture test green and silently re-open the
/// hole — the same "protection whose guard can be disabled" shape, one level down.
///
/// Asserted as a STRUCTURAL property (no line in the output begins a comment) rather
/// than against a specific sentence, so no docs edit anywhere can make it vacuous.
#[test]
fn read_applies_the_stripping() {
    let rel = "crates/app/src/server/api/admin/app_publish_tokens.rs";

    // Establish the premise before testing against it. "No output line is a comment" is
    // true by construction for a file that HAS no comments — so without this, the test
    // would quietly go vacuous the day someone strips the prose from that file, and
    // `read` could stop stripping with nothing going red. Depending on an unasserted
    // property of an unrelated file is the failure this whole file keeps re-learning.
    let raw = fs::read_to_string(repo_root().join(rel)).expect("fixture file is readable");
    assert!(
        raw.lines().any(|l| l.trim_start().starts_with("//")),
        "{rel} no longer contains line comments, so this test proves nothing about \
         whether `read` strips. Point it at a file that does."
    );

    let src = read(rel);
    let leaked: Vec<&str> = src
        .lines()
        .filter(|l| l.trim_start().starts_with("//"))
        .take(3)
        .collect();
    assert!(
        leaked.is_empty(),
        "`read` returned comment lines — it is no longer stripping, so every assertion \
         in this file is comment-satisfiable again. Leaked: {leaked:?}"
    );
    assert!(
        src.contains("pub(crate) fn router()"),
        "`read` stripped real code, not just comments"
    );
}

/// Both app route trees must carry the path-based scope middleware.
///
/// There are two, and that is the trap: `admin::apps::router()` under `/admin`, and the
/// parallel `/customer-apps` nest in `router/global.rs` that serves the same handlers
/// through a different mount. Gating only one leaves every route reachable unscoped by
/// the other path.
#[test]
fn both_app_route_trees_carry_the_scope_guard() {
    for (file, what) in [
        (
            "crates/app/src/server/api/admin/mod.rs",
            "the /admin/apps router",
        ),
        (
            "crates/app/src/server/router/global.rs",
            "the parallel /customer-apps nest",
        ),
    ] {
        let src = read(file);
        assert!(
            src.contains("app_scope_guard::enforce_app_scope"),
            "{what} ({file}) no longer applies `app_scope_guard::enforce_app_scope`.\n\
             Every `/{{id}}` custom-app route under it is now reachable by a platform \
             grant bounded to a DIFFERENT org — publish, rollback, secrets, access, all \
             of it. If the guard moved, point this test at its new home; if it was \
             removed, scope is no longer enforced and that needs to be a deliberate, \
             stated decision."
        );
    }
}

/// Every `batch_*` endpoint must fence its ids.
///
/// Batch ids arrive in the request BODY, where the path-based guard cannot see them, so
/// each one has to call `split_by_scope` itself. This is the exception most likely to be
/// forgotten by a new endpoint, and the most damaging to forget: `batch/delete` is a
/// cross-tenant delete with no discovery step.
#[test]
fn every_batch_endpoint_fences_its_ids_by_scope() {
    let src = read("crates/app/src/server/api/admin/apps/handlers.rs");

    let mut checked = 0;
    for chunk in src.split("pub async fn ").skip(1) {
        let name = chunk
            .split(['(', '<', ' ', '\n'])
            .next()
            .unwrap_or_default()
            .to_string();
        if !name.starts_with("batch_") {
            continue;
        }
        checked += 1;
        // The body ends at the next top-level item; scanning the whole chunk is fine
        // because the next `pub async fn` starts a new one.
        let body = chunk.split("\n}\n").next().unwrap_or(chunk);
        // Match the CALL, not the name. Each of these sites carries a comment reading
        // "see `split_by_scope`", so a bare substring check passes on the comment alone
        // and the test can never fail — verified by deleting a call and watching it stay
        // green. Requiring the open paren is what makes this assertion real.
        assert!(
            body.contains("split_by_scope("),
            "`{name}` does not call `split_by_scope(..)`.\n\
             Batch ids come from the request body, so `app_scope_guard` (which reads \
             `{{id}}` from the path) cannot see them. Without the call, a grant bounded \
             to one org operates on apps in every other one — no enumeration needed, the \
             caller just posts ids."
        );
    }

    assert!(
        checked >= 4,
        "expected at least the four known batch endpoints (publish / promote-latest / \
         unpublish / delete), found {checked}. If they were renamed, update this test; if \
         they moved to another file, this test is now scanning the wrong one and proving \
         nothing."
    );
}

/// The three documented scope exceptions must each keep their check.
///
/// These are the handlers whose target org is not in the path, listed in
/// `app_scope_guard`'s module docs and in `internal-docs/roles-and-authorization.md`.
#[test]
fn the_documented_scope_exceptions_keep_their_checks() {
    let handlers = read("crates/app/src/server/api/admin/apps/handlers.rs");
    let oxy_access = read("crates/app/src/server/api/admin/oxy_access.rs");

    // #1 — registration takes its org from the body.
    assert!(
        handlers.contains("deny_out_of_scope(&db, &user, req.org_id)")
            || handlers.contains("scope::deny_out_of_scope(&db, &user, req.org_id)"),
        "`create_app` no longer checks the body's `org_id` against the caller's scope. \
         A bounded grant could register an app in an org it cannot reach, then reach it \
         legitimately ever after — scope would be bypassable by creating your way in."
    );

    // #3 — the two listing routes have no single app to key on.
    assert!(
        handlers.contains("fn list_apps_scoped") && handlers.contains("apps::Column::OrgId.is_in"),
        "the apps registry no longer filters rows by scope; a bounded grant would see \
         every tenant's apps."
    );
    // Open paren for the same reason as `split_by_scope(` above: these files name their
    // helpers in comments, so a bare substring matches prose and asserts nothing.
    assert!(
        oxy_access.contains("scope_org_filter(&db, &user)"),
        "`oxy_access::list_grants` no longer filters by scope — the \"Add custom app\" \
         picker becomes a cross-tenant directory listing of every org's workspaces."
    );
}

/// Scope **write** paths must fail closed on an unreadable grant.
///
/// `scope_org_filter` deliberately collapses `Err` to "don't filter" — right for a
/// listing, catastrophic for `batch/delete`, where it would turn one transient `DbErr`
/// into a cross-tenant delete. The write paths therefore take the fallible
/// `scope_org_filter_checked`. This pins that they keep taking it: the lenient helper is
/// one autocomplete away, and the failure is silent.
#[test]
fn scope_write_paths_use_the_fallible_helper() {
    let src = read("crates/app/src/server/api/admin/apps/handlers.rs");

    // `deny_out_of_scope` moved to the shared `admin::scope` module when the org and
    // workspace handlers needed it too — a third copy would have been the wrong answer.
    // Its fail-closed behaviour is pinned by
    // `the_admin_scope_fence_refuses_rather_than_allowing`; what remains here is the
    // batch path, which is app-registry-shaped and stays local.
    for func in ["split_by_scope"] {
        let body = src
            .split(&format!("async fn {func}("))
            .nth(1)
            .unwrap_or_else(|| panic!("`{func}` not found — did it move or get renamed?"))
            .split("\n}\n")
            .next()
            .unwrap_or_default();
        assert!(
            body.contains("scope_org_filter_checked("),
            "`{func}` no longer calls `scope_org_filter_checked(..)`.\n\
             It is a WRITE path: an unreadable grant must refuse, not fall back to \
             \"unbounded\". The lenient `scope_org_filter` is for listings only — using \
             it here means a transient DbErr silently removes the scope fence."
        );
    }
}

/// Staff CI publish tokens are owned by their minter.
///
/// `Cap::ManageApps` is held by App Operators, so an unfiltered list plus an
/// id-addressed revoke would let a grant bounded to one tenant revoke every Oxy
/// engineer's CI token — cross-operator DoS with no boundary.
#[test]
fn publish_tokens_are_fenced_to_their_minter() {
    let src = read("crates/app/src/server/api/admin/app_publish_tokens.rs");
    assert!(
        src.contains("Column::CreatedBy.eq(actor.id)"),
        "`list_tokens` no longer filters to the caller's own tokens — every App \
         Operator can enumerate the whole staff token estate."
    );
    assert!(
        src.contains("token.created_by != Some(actor.id)"),
        "`revoke_token` no longer checks ownership — any holder of `manage_apps` can \
         revoke any Oxy engineer's CI publish token."
    );
    // The BARE name, no trailing paren. `read` has already dropped the two doc comments
    // that mention this capability in prose, so every remaining occurrence is code —
    // the paren was buying nothing and cost robustness: if the call ever wraps, rustfmt
    // puts `oxy_authz::Cap::OperatePlatform,` on its own line and the paren no longer
    // follows the name. Comment-stripping is what lets the needle be both short and
    // code-only, which was the point of adding it.
    assert!(
        src.contains("Cap::OperatePlatform"),
        "the cross-admin token view is no longer gated on `OperatePlatform` — with the \
         call gone, either nobody gets the shared view or everybody does, depending on \
         what replaced it."
    );
}

/// Every org-scoped route on the admin surface must resolve to a fenced handler.
///
/// Derived from the MERGE LIST, not from a hardcoded pair of routers. The previous
/// version derived handlers from each router — which was the right unit one level down,
/// and then re-introduced the hardcoded list one level up, naming `orgs_admin` and
/// `workspaces_admin` only. That is precisely why `org_subdomains` was invisible: a
/// second router merged on the SAME capability (`PlatformOrgs`), owning an
/// `/orgs/{org_id}` write that could disable another tenant's subdomain, and never part
/// of any sweep. `admin::metrics` was the same story on `PlatformOperate`.
///
/// `staff_surface` names every router and its gate, and is as parseable as the
/// `.route(` blocks already are. Enumerating from there means a fifth router fails on
/// the day it is merged rather than the day someone greps.
#[test]
fn every_org_scoped_route_resolves_to_a_fenced_handler() {
    let admin_mod = read("crates/app/src/server/api/admin/mod.rs");
    let merge_block = admin_mod
        .split("let staff_surface =")
        .nth(1)
        .expect("staff_surface not found in admin/mod.rs")
        .split(";\n")
        .next()
        .unwrap_or_default();

    // `x::router()` for every merged module, minus the owner-strict ones: those are
    // reachable only by the Global Owner, who is unbounded by definition.
    let owner_strict = ["billing", "app_admins"];
    let mut routers: Vec<String> = Vec::new();
    for piece in merge_block.split("::router()").take(32) {
        if let Some(name) = piece.rsplit(['(', ' ', '.', '\n']).next() {
            let name = name.trim();
            if !name.is_empty() && !owner_strict.contains(&name) && name != "routes" {
                routers.push(name.to_string());
            }
        }
    }
    assert!(
        routers.len() >= 8,
        "parsed only {} routers from staff_surface — the merge block's shape changed and \
         this test is now scanning almost nothing. Fix the parse before trusting a pass.",
        routers.len()
    );

    // Handlers can live outside their router's file (the logo trio, and `org_logo` is
    // reached through `orgs_admin`), so search the admin tree plus its delegates.
    let searched: Vec<String> = routers
        .iter()
        .map(|r| format!("crates/app/src/server/api/admin/{r}.rs"))
        .chain(["crates/app/src/server/api/org_logo.rs".to_string()])
        .filter(|p| repo_root().join(p).exists())
        .collect();
    let bodies: String = searched
        .iter()
        .map(|f| read(f))
        .collect::<Vec<_>>()
        .join("\n");

    for file in &searched {
        let src = read(file);
        let Some(router) = src.split("fn router()").nth(1) else {
            continue;
        };
        let router = router.split("\n}\n").next().unwrap_or_default();

        // Per `.route(..)` CHUNK, not per line: the `/orgs/{org_id}/logo` mount spans
        // four lines (path on one, three handlers after), so a line-wise scan missed it
        // and passed a mutation that unfenced the upload handler.
        for chunk in router.split(".route(").skip(1) {
            let chunk = chunk.split("\n        )").next().unwrap_or(chunk);
            if !chunk.contains("{org_id}") && !chunk.contains("{workspace_id}") {
                continue;
            }
            for verb in ["get(", "post(", "patch(", "delete(", "put("] {
                for piece in chunk.split(verb).skip(1) {
                    let name = piece
                        .split([')', ',', '.', '\n'])
                        .next()
                        .unwrap_or_default()
                        .rsplit("::")
                        .next()
                        .unwrap_or_default()
                        .trim();
                    if name.is_empty() || name.starts_with('|') {
                        continue;
                    }
                    let Some(body) = bodies
                        .split(&format!("fn {name}("))
                        .nth(1)
                        .map(|b| b.split("\n}\n").next().unwrap_or_default())
                    else {
                        panic!(
                            "`{name}` is mounted on an org-scoped route in {file} but is not \
                             in this test's search space. A handler this test cannot see is \
                             a handler nothing pins."
                        );
                    };
                    assert!(
                        body.contains("deny_out_of_scope"),
                        "`{name}` is mounted on an org-scoped route in {file} and does not \
                         fence. A bounded grant reaches every other tenant through it."
                    );
                }
            }
        }
    }
}

/// Every scoped admin WRITE must fence before it touches the database.
///
/// `platform_cap_guard` cannot do this: it decides on `Resource::platform()`, which has
/// no org, so a **bounded** grant passes every capability gate on the console and the
/// handler is the only thing left. That has now been got wrong three times — the app
/// registry, org membership, and org/workspace administration, where
/// `DELETE /admin/orgs/{any}` ran completely unfenced.
///
/// Each fix shipped without a pin, and the last round's blocker was found only because a
/// reviewer ran `grep -c platform_reaches` by hand. This is that grep, mechanised.
///
/// The `before db.begin(` ordering matters as much as the presence: a fence after the
/// transaction opens still refuses, but it has already done work on a tenant the caller
/// cannot reach.
#[test]
fn scoped_admin_writes_fence_before_touching_the_database() {
    // (file, handler, whether the fence must precede a transaction)
    let sites: &[(&str, &[&str])] = &[
        (
            "crates/app/src/server/api/admin/users_admin.rs",
            &["add_to_org", "update_role", "remove_from_org"],
        ),
        (
            // The most destructive verbs on the console. `delete_org` had no lookup at
            // all — it went straight to `delete_by_id`.
            "crates/app/src/server/api/admin/orgs_admin.rs",
            &[
                "delete_org",
                "rename_org",
                "transfer_ownership",
                "get_org_detail",
            ],
        ),
        (
            "crates/app/src/server/api/admin/workspaces_admin.rs",
            &[
                "delete_workspace",
                "update_workspace",
                "transfer_org",
                "get_workspace_detail",
            ],
        ),
        (
            // Named explicitly because the router sweep above cannot see this
            // one: it resolves `admin/{router}.rs`, and `airway_config` is a
            // DIRECTORY module (`airway_config/mod.rs` + `handlers.rs`) — as are
            // `apps`, `partners` and `workspace_health`. That blindness is
            // exactly how these two shipped unfenced: a bounded grant could
            // `PUT /admin/airway/config/{kind}/workspaces/{any}` and pin another
            // tenant's pipelines to a policy that halts them.
            //
            // Widening the sweep to directories is the better fix and is a
            // bigger change than this one: the other three would come with it,
            // their handlers live in sub-modules the sweep's search space does
            // not reach, and its route-chunk parse assumes one indent level. So
            // this list carries airway_config today, and the gap is written
            // down rather than left to be rediscovered.
            "crates/app/src/server/api/admin/airway_config/handlers.rs",
            &["put_workspace_override", "delete_workspace_override"],
        ),
        (
            // The OLTP console. The handlers themselves are in `oxy-oltp`,
            // which sits below `oxy-app` and cannot import the fence — so this
            // file is a shim whose whole purpose is to hold it. Unfenced, a
            // bounded grant reached `POST …/credentials` (a write DSN for any
            // tenant) and `DELETE …/oltp`, which destroys the database.
            //
            // `list_tenants` is absent deliberately: it is not org-keyed and
            // FILTERS by scope rather than refusing, which is the read-path
            // answer everywhere else on this console.
            "crates/app/src/server/api/admin/oltp.rs",
            &[
                "get_status",
                "provision",
                "set_visibility",
                "deprovision",
                // The per-writer sibling of `deprovision`. Listed because this
                // is where a reviewer looks for the destructive verbs, and the
                // two are one path segment apart while differing by the entire
                // tenant — the narrow one being absent here is exactly how it
                // would come to be read as the safe one.
                "deprovision_writer",
                "credentials",
            ],
        ),
        (
            // The Airhouse console, same shape as OLTP's: handlers in a crate
            // below `oxy-app`, routes and fence here. Workspace-keyed, so the
            // fence is `deny_out_of_scope_opt` — the needle below matches both.
            //
            // `list_fleet` is absent deliberately: it FILTERS by scope rather
            // than refusing, which is the read-path answer on this console.
            "crates/app/src/server/api/admin/airhouse.rs",
            &["provision"],
        ),
    ];

    for (file, handlers) in sites {
        let src = read(file);
        for handler in *handlers {
            let body = src
                .split(&format!("pub async fn {handler}("))
                .nth(1)
                .unwrap_or_else(|| panic!("`{handler}` not found in {file} — renamed or moved?"))
                .split("\n}\n")
                .next()
                .unwrap_or_default();

            // Bare name, no paren: workspaces call `deny_out_of_scope_opt(`, and `read`
            // has already stripped the comments that could otherwise satisfy it.
            let fence = body.find("deny_out_of_scope").unwrap_or_else(|| {
                panic!(
                    "`{handler}` ({file}) does not call `deny_out_of_scope(..)`.\n\
                     The capability guard cannot fence this — it decides on a resource with \
                     no org — so a grant bounded to one tenant reaches every other one here."
                )
            });

            // If the handler opens a transaction, the fence must come first.
            if let Some(begin) = body.find("db.begin(") {
                assert!(
                    fence < begin,
                    "`{handler}` ({file}) fences AFTER opening its transaction. It still \
                     refuses, but it has already done work against a tenant the caller \
                     cannot reach."
                );
            }
        }
    }
}

/// The shared fence must fail closed, and must not leak existence — **per function**.
///
/// File-wide needles were the bug here. `scope.rs` holds two functions and each
/// contributes a `NOT_FOUND` and an `INTERNAL_SERVER_ERROR`, so deleting
/// `deny_out_of_scope_opt`'s null-org refusal — reverting exactly the escape that arm
/// exists to close, since `Ok(_) => Ok(())` would then swallow it — left both file-wide
/// assertions green on the *other* function's copies. The router test didn't catch it
/// either: it asserts the call is present, and the call would still be there.
///
/// Same defect as rounds 3, 4 and 7, in its fourth costume: a needle satisfiable by
/// something other than the thing it names. Scoping to one function body is the
/// instrument this file already uses elsewhere.
#[test]
fn the_admin_scope_fence_refuses_rather_than_allowing() {
    let src = read("crates/app/src/server/api/admin/scope.rs");

    let body_of = |name: &str| -> &str {
        src.split(&format!("pub async fn {name}("))
            .nth(1)
            .unwrap_or_else(|| panic!("`{name}` not found in admin::scope"))
            .split("\n}\n")
            .next()
            .unwrap_or_default()
    };

    for name in ["deny_out_of_scope", "deny_out_of_scope_opt"] {
        let body = body_of(name);
        assert!(
            body.contains("StatusCode::INTERNAL_SERVER_ERROR"),
            "`{name}` no longer refuses on an unreadable grant — one transient DbErr \
             would hand out tenant Owner, or drop a tenant"
        );
        assert!(
            body.contains("StatusCode::NOT_FOUND"),
            "`{name}` no longer answers out-of-scope with 404"
        );
        assert!(
            !body.contains("StatusCode::FORBIDDEN"),
            "`{name}` returns 403 — 403 confirms the org exists, so a bounded operator \
             can map the tenant directory by probing ids"
        );
    }

    // The specific arm, not just "a NOT_FOUND somewhere in the function": a null org is
    // by definition not in `Scope::Orgs(..)`, and without this arm the catch-all
    // `Ok(_) => Ok(())` turns an org-less workspace into a hole for every bounded grant.
    assert!(
        body_of("deny_out_of_scope_opt").contains("Scope::Orgs(_)"),
        "`deny_out_of_scope_opt` no longer refuses a NULL org for a bounded grant. The \
         call sites still call it, so nothing else in this suite will notice."
    );
}

/// Scope denials must be indistinguishable from "no such app".
///
/// A 403 confirms the app exists, which lets a bounded operator enumerate the registry
/// one uuid at a time — the guard would fence the data and leak the index.
#[test]
fn out_of_scope_reads_are_not_found_rather_than_forbidden() {
    let guard = read("crates/app/src/server/api/middlewares/app_scope_guard.rs");
    assert!(
        guard.contains("StatusCode::NOT_FOUND"),
        "`app_scope_guard` no longer answers out-of-scope requests with 404"
    );
    assert!(
        !guard.contains("StatusCode::FORBIDDEN"),
        "`app_scope_guard` returns 403 somewhere. A scope denial must be a 404: 403 \
         confirms the app exists, so a bounded operator can map the whole registry by \
         probing ids."
    );
}

/// Every failure inside the guard must REFUSE, never pass through unscoped.
///
/// The guard has two ways to fail on one root cause — the pool not handing back a
/// connection, and the app lookup erroring. They are the same outage; which one a
/// request hits is timing. For a while these disagreed: the pool arm deferred
/// ("the handler needs the same connection and will fail honestly") while the query arm
/// refused. That reasoning outsources the fence's safety to every handler behind it,
/// including ones not written yet, and it meant a DB blip either refused or admitted an
/// unscoped request depending on where the pool gave out.
///
/// Counted from both directions, because one direction alone misses the case that
/// matters. `refusals >= 2` goes red if either existing arm reverts to a pass-through —
/// but a THIRD failure arm added later as a pass-through leaves it green, which is
/// precisely the shape this test exists to catch. So the pass-throughs are pinned to an
/// exact count too: adding one fails loudly and points at the list to update.
#[test]
fn every_failure_arm_in_the_scope_guard_refuses() {
    let guard = read("crates/app/src/server/api/middlewares/app_scope_guard.rs");

    let refusals = guard.matches("StatusCode::INTERNAL_SERVER_ERROR").count();
    assert!(
        refusals >= 2,
        "expected both failure arms (no connection, and an errored app lookup) to refuse \
         with 500; found {refusals}. A failure that passes through hands an UNSCOPED \
         request to a handler that trusts this layer to have fenced it."
    );

    // The three legitimate "nothing to check" exits — unauthenticated (the outer auth
    // layer owns that verdict), no `{id}` in the matched path, and no such app — plus
    // the success tail. Every one of those is a request going through UNFENCED for a
    // reason that has been argued; a fifth is one that hasn't.
    let pass_throughs = count_pass_throughs(&guard);
    assert_eq!(
        pass_throughs, 4,
        "the number of unfenced exits from `app_scope_guard` changed (expected 4: \
         unauthenticated, no path id, no such app, and the success tail). If you added a \
         pass-through, say why it is safe here and update this count; if you removed \
         one, update it too — this number is the list of ways a request leaves this \
         layer without being scoped.\n\
         \n\
         If you just RENAMED the request binding, that is this test's blind spot rather \
         than a real change: it counts the literal `Ok(next.run(request).await)`. Update \
         `count_pass_throughs`."
    );
}

/// Counts the ways a request leaves a guard without a verdict.
///
/// Matches the literal call shape, so renaming the request binding (`request` → `req`)
/// makes this read zero and the callers fail. That is the right direction — loud, not
/// silent — but the failure looks like "you added exits" rather than "you renamed a
/// variable", so both call sites say so in their message.
fn count_pass_throughs(src: &str) -> usize {
    src.matches("Ok(next.run(request).await)").count()
}

/// `platform_cap_guard` must refuse when standing is unreadable, not fall back.
///
/// This guard's fail direction MOVED in this branch: it used to defer to the legacy
/// oracle (`is_oxy_owner || is_oxy_app_admin`), which was safe while that oracle answered
/// the same question the guard asks. It stopped being safe when `app_admins` gained App
/// Operator rows — the oracle became strictly broader than the model, so deferring
/// promoted an App Operator to every console section. Reverting that one line went green
/// across the whole suite, because nothing pinned it. This is that pin.
///
/// Counted in both directions for the same reason as the scope guard: a refusal that
/// reverts shows up as a missing 500, and a NEW unfenced exit shows up in the count.
#[test]
fn the_capability_guard_refuses_on_unreadable_standing() {
    let guard = read("crates/app/src/server/api/middlewares/platform_cap_guard.rs");

    assert!(
        guard.contains("StatusCode::INTERNAL_SERVER_ERROR"),
        "`platform_cap_guard` no longer refuses when platform standing is unreadable. \
         If it fell back to the legacy oracle, that oracle is true for App Operators \
         too — so an unreadable standing would GRANT `PlatformOrgs`, `PlatformOperate` \
         and every other section, and internal jobs / compiles / the explorer have no \
         second gate behind this layer."
    );

    let pass_throughs = count_pass_throughs(&guard);
    assert_eq!(
        pass_throughs, 2,
        "the number of unfenced exits from `platform_cap_guard` changed (expected 2: \
         the Global Owner short-circuit, whose standing is an env read no outage may \
         revoke, and the success tail). A third exit needs to argue for itself here.\n\
         \n\
         If you just RENAMED the request binding, see `count_pass_throughs` — that is \
         this test's blind spot, not a real change."
    );
}

/// The grant table is capability-gated, and every handler on it carries the row fence.
///
/// The capability gate alone is not the control: it answers "may you administer grants",
/// which a bounded Global Admin passes, and says nothing about *which* rows. Losing the
/// per-handler `may_delegate` call restores exactly the escalation the owner-only guard
/// existed to prevent — one Global Admin re-scoping themselves to every tenant.
///
/// The unit tests in `admin/mod.rs` cannot see this. They build their own router and
/// mount their own probe, so they prove the middleware works and nothing about what
/// ships; that fixture asserted `app_admins` was owner-strict for one release after it
/// stopped being mounted that way, and passed the whole time.
#[test]
fn the_grant_table_is_capability_gated_and_every_handler_is_row_fenced() {
    let modrs = read("crates/app/src/server/api/admin/mod.rs");

    let mount = modrs
        .split("app_admins::router()")
        .nth(1)
        .expect("app_admins is no longer mounted in admin::router")
        .split(')')
        .next()
        .unwrap_or_default();
    assert!(
        mount.contains("Action::PlatformGrants"),
        "app_admins must mount under `cap(Action::PlatformGrants)`; found `{mount}`"
    );

    let src = read("crates/app/src/server/api/admin/app_admins.rs");
    let body_of = |name: &str| -> &str {
        src.split(&format!("pub async fn {name}("))
            .nth(1)
            .unwrap_or_else(|| panic!("`{name}` not found in app_admins"))
            .split("\n}\n")
            .next()
            .unwrap_or_default()
    };

    // Per function, not file-wide: `app_admins.rs` has three handlers and a file-wide
    // needle is satisfied by any one of them. That is the defect this suite has now
    // found four times.
    for name in ["create_app_admin", "delete_app_admin"] {
        assert!(
            body_of(name).contains("delegatable("),
            "`{name}` writes the grant table without asking the delegation bound — a \
             Global Admin can re-role or revoke a peer, or widen their own grant"
        );
        assert!(
            body_of(name).contains("delegation::refuse"),
            "`{name}` computes a verdict but never refuses on it"
        );
    }
    // `delegatable` is a spelling; this is the assertion that it still routes to the
    // model rather than having grown a local opinion about who out-ranks whom.
    assert!(
        src.split("fn delegatable(")
            .nth(1)
            .is_some_and(|b| b.contains("may_delegate(facts")),
        "`delegatable` no longer defers to `oxy_authz::may_delegate` — the bound is now \
         restated in the handler layer, which is the scatter this model ended"
    );

    // The upsert authorizes TWO pairs: the values being written, and the grant being
    // overwritten. Checking only the first lets a Global Admin demote a peer by POSTing
    // an admissible role over their row. Two call sites is the observable difference.
    let create = body_of("create_app_admin");
    assert!(
        create.matches("delegation::refuse").count() >= 2,
        "`create_app_admin` refuses only once — an upsert that replaces an existing \
         grant must authorize the row it destroys as well as the one it writes"
    );

    // Revoke has no caller-supplied (role, scope), so reading the target row IS the
    // check. A delete_by_id that never loads the row cannot have made one.
    let delete = body_of("delete_app_admin");
    assert!(
        delete.find("find_by_id").is_some_and(|read_at| delete
            .find("delete_by_id")
            .is_some_and(|del_at| read_at < del_at)),
        "`delete_app_admin` must read the target grant BEFORE deleting it — the row's \
         own (role, scope) is the only thing there is to authorize against"
    );

    // Both writes are audited, in-transaction. A grant is the authority to reach other
    // tenants; "who made this person staff" must not answer "check the logs".
    for name in ["create_app_admin", "delete_app_admin"] {
        assert!(
            body_of(name).contains("audit::record_in_txn"),
            "`{name}` does not record an in-transaction audit row — a best-effort or \
             absent one means the grant can change with nothing recording it"
        );
    }
}

/// An unnameable stored role denies everyone EXCEPT the Global Owner.
///
/// `PlatformRole::from_str` returns `None` for a role this build cannot expand — the
/// state a rollback past a role's introduction produces. The loader already drops such a
/// grant; if the delegation path treated it as absent, or defaulted it to the weakest
/// role, whoever out-ranks that default could revoke rows they must not touch.
///
/// The owner half is the one that shipped wrong.
/// `match stored_standing(..) { Some(..) => may_delegate(..), None => Err(..) }` was the
/// obvious spelling and it locked root out: the `None` arm returns before anything
/// consults `is_global_owner`, so the short-circuit inside `may_delegate` is never
/// reached. After a rollback past a role's introduction nobody — not even the owner —
/// could revoke the affected grants, and the only remedy was hand-written SQL. The
/// comment above `stored_standing` claimed those rows froze "for everyone but the
/// owner", which is precisely the wrong half.
#[test]
fn an_unexpandable_stored_role_denies_everyone_but_the_owner() {
    let src = read("crates/app/src/server/api/admin/app_admins.rs");
    let body = src
        .split("fn delegatable(")
        .nth(1)
        .expect("`delegatable` is gone — the owner short-circuit lived in it")
        .split("\n}\n")
        .next()
        .unwrap_or_default();

    let owner_arm = body
        .find("None if facts.is_global_owner")
        .expect("`delegatable` no longer lets the Global Owner through on an unnameable role");
    let deny_arm = body
        .find("None => Err(")
        .expect("`delegatable` no longer denies a non-owner an unnameable role");
    assert!(
        owner_arm < deny_arm,
        "the owner arm must precede the catch-all deny, or it is unreachable — which is \
         the exact defect this test exists for"
    );

    // One decision function, used by every caller. Two copies is how the list came to
    // disagree with the writes about whether a row was writable.
    for caller in ["create_app_admin", "delete_app_admin", "list_app_admins"] {
        let f = src
            .split(&format!("pub async fn {caller}("))
            .nth(1)
            .unwrap_or_else(|| panic!("`{caller}` not found"))
            .split("\n}\n")
            .next()
            .unwrap_or_default();
        assert!(
            f.contains("delegatable("),
            "`{caller}` decides delegation without `delegatable`, so it does not inherit \
             the owner short-circuit"
        );
    }
}

/// Both write paths authorize against a row they hold a lock on, and neither reaches for
/// a second pool connection while holding it.
///
/// `create` closed this and `delete` did not, which is the worse half: a concurrent
/// promotion between an unlocked read and the delete turns an authorized revoke of an
/// `app_operator` into an unauthorized revoke of a `global_admin`. Reading the row IS
/// the entire check for a revoke, so reading one the delete does not hold is not a check.
#[test]
fn both_grant_writes_lock_their_target_and_hold_one_connection() {
    let src = read("crates/app/src/server/api/admin/app_admins.rs");
    let body_of = |name: &str| -> &str {
        src.split(&format!("pub async fn {name}("))
            .nth(1)
            .unwrap_or_else(|| panic!("`{name}` not found"))
            .split("\n}\n")
            .next()
            .unwrap_or_default()
    };

    for name in ["create_app_admin", "delete_app_admin"] {
        let body = body_of(name);
        let begin = body
            .find("db.begin()")
            .unwrap_or_else(|| panic!("`{name}` no longer opens a transaction"));
        let lock = body
            .find("lock_exclusive()")
            .unwrap_or_else(|| panic!("`{name}` reads the target grant without a row lock"));
        assert!(
            begin < lock,
            "`{name}` locks before opening its transaction, so the lock is not held for \
             the write — the read and the write must sit in one transaction"
        );

        // And the caller's OWN standing is loaded before any of it.
        //
        // `actor_facts` goes to `&db`, so calling it inside the transaction borrows a
        // second pool connection while this request holds one plus a locked row. N
        // concurrent revokes against an N-connection pool then wedge each other until
        // the acquire timeout. `invalidate_admin_cache()` runs after every write here, so
        // the TTL cache is cold exactly when concurrent writes are most likely — the
        // miss is the common path, not the rare one.
        //
        // Pinned because nothing else can catch it: the decision is identical either
        // way, so the regression produces no wrong answer, no failing assertion
        // anywhere else in this suite, and no symptom at all until the pool saturates.
        let facts = body
            .find("actor_facts(")
            .unwrap_or_else(|| panic!("`{name}` no longer loads the actor's standing"));
        assert!(
            facts < begin,
            "`{name}` loads the actor's standing inside its transaction — that is an \
             `&db` await holding `FOR UPDATE`, so it borrows a second pool connection \
             while pinning a row"
        );
    }
}

/// A re-scope must be legible in the audit trail.
///
/// The `before` payload carried `{role, scope_all}` only, so `Orgs([acme])` →
/// `Orgs([globex])` recorded an identical shape on both sides with no way to see what
/// the grant used to reach. Re-scoping is the most likely edit this console sees — it is
/// the one the change's own end-to-end verification performed.
#[test]
fn the_grant_audit_records_scope_on_both_sides() {
    let src = read("crates/app/src/server/api/admin/app_admins.rs");
    let body_of = |name: &str| -> &str {
        src.split(&format!("pub async fn {name}("))
            .nth(1)
            .unwrap_or_else(|| panic!("`{name}` not found"))
            .split("\n}\n")
            .next()
            .unwrap_or_default()
    };

    let create = body_of("create_app_admin");
    let change = create
        .split(".change(")
        .nth(1)
        .expect("`create_app_admin` no longer records a before/after");
    // The quoted JSON key, not the bare identifier: the identifier also names the local
    // bindings and the response field further down the function, which is how this
    // counted five and proved nothing.
    assert_eq!(
        change.matches("\"scope_org_ids\":").count(),
        2,
        "`create_app_admin`'s audit entry must carry `scope_org_ids` on BOTH sides; a \
         re-scope is invisible with it on only one"
    );

    let delete = body_of("delete_app_admin");
    let deleted_change = delete
        .split(".change(")
        .nth(1)
        .expect("`delete_app_admin` no longer records a before/after");
    assert!(
        deleted_change.contains("\"scope_org_ids\":"),
        "`delete_app_admin`'s audit entry must record what the revoked grant reached"
    );
}

/// A denial reaches the client with its reason.
///
/// `DelegationDenial::as_str` is documented as "carried out to the API so the console can
/// say which bound was hit", contrasted against a plain forbidden that "sends the
/// operator to ask someone why". A bare `StatusCode::FORBIDDEN` cannot keep that promise:
/// the frontend reads `err.response.data.message` and rendered the axios default.
#[test]
fn a_delegation_denial_carries_its_reason_to_the_client() {
    let src = read("crates/app/src/server/api/admin/delegation.rs");
    assert!(
        src.contains("impl axum::response::IntoResponse for Refusal"),
        "`Refusal` no longer renders itself, so the denial cannot reach the client"
    );
    assert!(
        src.contains("\"message\":"),
        "the refusal body must carry `message` — the field the console's error handler \
         already reads"
    );
    assert!(
        src.contains("\"denial\":"),
        "the refusal body must carry the stable `denial` id for clients that branch"
    );
    // The helper the docs used to name. Its absence is deliberate: it took a
    // `&DatabaseConnection` and so could not run against the locked row inside a
    // transaction, which is the check that matters.
    assert!(
        !src.contains("pub async fn deny_undelegatable"),
        "`deny_undelegatable` is back — no correct handler can call it, and naming it in \
         the docs sends the next author around the row lock"
    );
}

/// Every **read** on the airway admission console that returns per-tenant rows
/// must narrow them to the caller's platform scope.
///
/// The write half is mechanised above; this is its counterpart, and it exists
/// because the read half shipped with a hole. `platform_cap_guard` decides on
/// `Resource::platform()`, so a bounded `global_admin` reaches this console and
/// the handler is the only thing left — and `preview_policy` did not even take
/// an `AuthenticatedUserExtractor`, so a grant bounded to two orgs could
/// enumerate every tenant's airway pipelines: `pipeline_ref` is
/// `{workspace_id}:{real path}`, `resource` names tables inside their
/// pipelines, and an `unevaluated` entry quotes their `.airway.yml`.
///
/// What made that a defect rather than a judgement call is that three doc
/// blocks shipped alongside the *other* fences claimed the surface was
/// covered. A doc that reads as coverage is worse than none, so the claim is
/// pinned here instead of restated there.
///
/// Asserted per handler body, not file-wide: `handlers.rs` and `preview.rs`
/// each hold several functions, and a file-wide needle is satisfiable by the
/// wrong one — the defect this file has now re-learned four times.
#[test]
fn scoped_admin_reads_on_the_airway_console_filter_by_scope() {
    let sites: &[(&str, &str)] = &[
        (
            "crates/app/src/server/api/admin/airway_config/handlers.rs",
            "get_config",
        ),
        (
            "crates/app/src/server/api/admin/airway_config/preview.rs",
            "preview_policy",
        ),
    ];

    for (file, handler) in sites {
        let src = read(file);
        let body = src
            .split(&format!("pub async fn {handler}("))
            .nth(1)
            .unwrap_or_else(|| panic!("`{handler}` not found in {file} — renamed or moved?"))
            .split("\n}\n")
            .next()
            .unwrap_or_default();

        // The extractor is half of it: without an actor there is nothing to
        // resolve a scope from, which is exactly how `preview_policy` shipped.
        assert!(
            body.contains("AuthenticatedUserExtractor"),
            "`{handler}` ({file}) takes no `AuthenticatedUserExtractor`, so it cannot know \
             who is asking. The capability guard decides on a resource with no org, so a \
             grant bounded to one tenant reads every other one here."
        );
        // And the shared read-path filter is the other half — the same one
        // `admin::apps::handlers` owns, never a second spelling of it.
        assert!(
            body.contains("scope_org_filter"),
            "`{handler}` ({file}) does not call `scope_org_filter(..)`. Every read on this \
             console that returns per-tenant rows narrows them; the docs in \
             `airway_config/mod.rs` and `handlers.rs` state that as a property of the \
             whole surface."
        );
    }
}
