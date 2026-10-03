//! `admin::scope` asked directly: every fence and the listing scope, for every kind
//! of caller.
//!
//! The handler cases next door prove each surface narrows. These pin the two halves
//! of the rule against each other, so a change to how the grant is *read* that moves
//! any caller's answer — by id or in a listing — fails here, not three handlers later.
//!
//! Each case collects every answer one caller gets into one value and compares it
//! whole: a fence that drifted from the listing shows up as one field of one diff.

use axum::http::StatusCode;
use oxy_app::server::api::admin::scope::{
    deny_out_of_scope, deny_out_of_scope_for_workspace, deny_out_of_scope_opt,
    deny_out_of_scope_platform, list_scope,
};
use oxy_auth::types::AuthenticatedUser;
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::fixture::{World, seed_user, world};

const NOT_FOUND: Result<(), StatusCode> = Err(StatusCode::NOT_FOUND);
const REFUSED: Result<(), StatusCode> = Err(StatusCode::INTERNAL_SERVER_ERROR);

/// Everything `admin::scope` answers one caller.
#[derive(Debug, PartialEq)]
struct Answers {
    org_a: Result<(), StatusCode>,
    org_b: Result<(), StatusCode>,
    /// `deny_out_of_scope_opt(.., None)` — an org-less row.
    no_org: Result<(), StatusCode>,
    ws_a: Result<(), StatusCode>,
    ws_b: Result<(), StatusCode>,
    ws_orphan: Result<(), StatusCode>,
    /// A workspace id that does not exist.
    ws_missing: Result<(), StatusCode>,
    /// A fleet-wide action.
    platform: Result<(), StatusCode>,
    listing: Result<Option<Vec<Uuid>>, StatusCode>,
}

impl Answers {
    /// What an unbounded reader gets: everything that exists, and no narrowing.
    fn everything() -> Self {
        Self {
            org_a: Ok(()),
            org_b: Ok(()),
            no_org: Ok(()),
            ws_a: Ok(()),
            ws_b: Ok(()),
            ws_orphan: Ok(()),
            ws_missing: NOT_FOUND,
            platform: Ok(()),
            listing: Ok(None),
        }
    }
}

async fn answers(w: &World, who: &AuthenticatedUser) -> Answers {
    let db = &w.db;
    Answers {
        org_a: deny_out_of_scope(db, who, w.org_a).await,
        org_b: deny_out_of_scope(db, who, w.org_b).await,
        no_org: deny_out_of_scope_opt(db, who, None).await,
        ws_a: deny_out_of_scope_for_workspace(db, who, w.ws_a).await,
        ws_b: deny_out_of_scope_for_workspace(db, who, w.ws_b).await,
        ws_orphan: deny_out_of_scope_for_workspace(db, who, w.ws_orphan).await,
        ws_missing: deny_out_of_scope_for_workspace(db, who, Uuid::new_v4()).await,
        platform: deny_out_of_scope_platform(db, who).await,
        listing: list_scope(db, who).await,
    }
}

#[tokio::test]
async fn a_bounded_grant_reaches_its_orgs_and_nothing_else() {
    let w = world().await;
    assert_eq!(
        answers(&w, &w.bounded).await,
        Answers {
            org_a: Ok(()),
            org_b: NOT_FOUND,
            no_org: NOT_FOUND,
            ws_a: Ok(()),
            ws_b: NOT_FOUND,
            ws_orphan: NOT_FOUND,
            ws_missing: NOT_FOUND,
            platform: NOT_FOUND,
            listing: Ok(Some(vec![w.org_a])),
        },
        "a grant bounded to org A: the fences and the listing must name the same reach"
    );
}

#[tokio::test]
async fn an_all_orgs_grant_and_the_owner_reach_everything() {
    let w = world().await;
    for (label, who) in w.everything_readers() {
        assert_eq!(
            answers(&w, who).await,
            Answers::everything(),
            "{label}: unbounded by every fence and unnarrowed in every listing"
        );
    }
}

/// No standing at all — no grant row, not in `OXY_OWNER`.
///
/// Unreachable over HTTP: `platform_cap_guard` refuses a caller with no standing
/// before any of these handlers runs. Pinned anyway, because the two halves answer
/// it differently — a fence lets it through (the defensive default `deny_out_of_scope`
/// documents), a listing narrows it to nothing — and a change to how the grant is
/// read must not move either answer without someone deciding to.
#[tokio::test]
async fn no_standing_passes_a_fence_and_lists_nothing() {
    let w = world().await;
    let nobody = seed_user(&w.db, "nobody@staff-scope.test").await;
    assert_eq!(
        answers(&w, &nobody).await,
        Answers {
            listing: Ok(Some(Vec::new())),
            ..Answers::everything()
        },
    );
}

/// A grant that cannot be read refuses — never reads as unbounded — on every fence
/// and on the listing.
///
/// The grant table is renamed out from under the lookup, so reading a grant errors
/// while every other table still answers: the workspace lookup behind
/// `_for_workspace` runs, and a missing workspace is still a plain 404. Nothing in
/// this process read a grant before the rename, so no answer comes from the grant
/// cache.
#[tokio::test]
async fn an_unreadable_grant_fails_closed() {
    let w = world().await;
    w.db.execute_unprepared("ALTER TABLE app_admins RENAME TO app_admins_unreadable")
        .await
        .expect("take the grant table away");

    for (label, who) in [
        ("a bounded grant", &w.bounded),
        ("an all-orgs grant", &w.unbounded),
    ] {
        assert_eq!(
            answers(&w, who).await,
            Answers {
                org_a: REFUSED,
                org_b: REFUSED,
                no_org: REFUSED,
                ws_a: REFUSED,
                ws_b: REFUSED,
                ws_orphan: REFUSED,
                ws_missing: NOT_FOUND,
                platform: REFUSED,
                listing: Err(StatusCode::INTERNAL_SERVER_ERROR),
            },
            "{label}, unreadable: must refuse everywhere, never fall back to unbounded"
        );
    }

    // The Global Owner's standing is the `OXY_OWNER` allow-list, an env read that
    // no outage takes away — the rule `platform_cap_guard` states — so every fence
    // still passes. The listing reads standing through the loader, which needs the
    // grant table, and refuses. That asymmetry predates this test; it is recorded
    // here so that moving it is a decision, not a side effect.
    assert_eq!(
        answers(&w, &w.owner).await,
        Answers {
            listing: Err(StatusCode::INTERNAL_SERVER_ERROR),
            ..Answers::everything()
        },
        "the Global Owner, grant table unreadable"
    );
}
