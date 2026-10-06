//! The flat routes under a grant-bound token (design §4.5): discovery lists
//! only what the grants cover, and a route that answers from raw membership is
//! refused outright — while a session and an all-access token see exactly what
//! they always did.
//!
//! The user here belongs to two orgs, A (two workspaces) and B, so "everything
//! the user can reach" and "what the token was granted" are different sets.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use oxy_authz::RoleCeiling;
use serde_json::json;
use uuid::Uuid;

use super::stack::{
    Reach, flat_api, get_as, get_in_session, ids, join_org, mint, org_grant, published_app,
    workspace_grant,
};
use super::{Fixture, call, fixture, seed_org, seed_workspace_in};

struct TwoOrgs {
    fx: Fixture,
    /// Org A is the fixture's org; `a1` is the fixture's workspace.
    a1: Uuid,
    a2: Uuid,
    org_b: Uuid,
    b1: Uuid,
}

async fn two_orgs() -> TwoOrgs {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let a2 = seed_workspace_in(&fx.db, fx.org_id).await;
    let org_b = seed_org(&fx.db).await;
    join_org(&fx.db, org_b, fx.user.id, OrgRole::Owner).await;
    let b1 = seed_workspace_in(&fx.db, org_b).await;
    TwoOrgs {
        a1: fx.workspace_id,
        a2,
        org_b,
        b1,
        fx,
    }
}

fn sorted(mut ids: Vec<Uuid>) -> Vec<Uuid> {
    ids.sort();
    ids
}

#[tokio::test]
async fn a_grant_bound_token_discovers_only_the_orgs_and_workspaces_it_covers() {
    let t = two_orgs().await;
    let (org_a, org_b) = (t.fx.org_id, t.org_b);
    let (_, secret) = mint(
        &t.fx.db,
        t.fx.user.id,
        Reach::granted(vec![workspace_grant(org_a, t.a1, RoleCeiling::Member)]),
    )
    .await;

    // The session sees both orgs and both of A's workspaces.
    let (_, orgs) = get_in_session(&t.fx, "/orgs").await;
    assert_eq!(sorted(ids(&orgs)), sorted(vec![org_a, org_b]));
    let (_, workspaces) = get_in_session(&t.fx, &format!("/orgs/{org_a}/workspaces")).await;
    assert_eq!(sorted(ids(&workspaces)), sorted(vec![t.a1, t.a2]));

    // The token sees the one org and the one workspace it was granted.
    let (status, orgs) = get_as(&secret, "/orgs").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&orgs), vec![org_a]);
    let (status, workspaces) = get_as(&secret, &format!("/orgs/{org_a}/workspaces")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&workspaces), vec![t.a1]);

    // An org it holds no grant in does not exist.
    assert_eq!(
        get_as(&secret, &format!("/orgs/{org_b}/workspaces"))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    // And a workspace grant is not an org grant: the org's own routes need one
    // that covers every workspace.
    assert_eq!(
        get_as(&secret, &format!("/orgs/{org_a}")).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn an_org_wide_grant_reaches_the_org_routes_at_its_ceiling() {
    let t = two_orgs().await;
    let (org_a, org_b) = (t.fx.org_id, t.org_b);
    let (_, secret) = mint(
        &t.fx.db,
        t.fx.user.id,
        Reach::granted(vec![org_grant(org_a, RoleCeiling::Viewer)]),
    )
    .await;

    let (status, workspaces) = get_as(&secret, &format!("/orgs/{org_a}/workspaces")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        sorted(ids(&workspaces)),
        sorted(vec![t.a1, t.a2]),
        "every workspace in the org, including ones created later"
    );

    let (status, org) = get_as(&secret, &format!("/orgs/{org_a}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        org["role"], "member",
        "an Owner under a viewer ceiling acts as a Member on org routes"
    );
    let (_, org) = get_in_session(&t.fx, &format!("/orgs/{org_a}")).await;
    assert_eq!(org["role"], "owner");

    assert_eq!(
        get_as(&secret, &format!("/orgs/{org_b}")).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn my_apps_lists_only_the_orgs_a_grant_touches() {
    let t = two_orgs().await;
    let app_a = published_app(&t.fx.db, t.fx.org_id, t.a1).await.id;
    let app_b = published_app(&t.fx.db, t.org_b, t.b1).await.id;

    let (status, apps) = get_in_session(&t.fx, "/apps/mine").await;
    assert_eq!(status, StatusCode::OK, "{apps}");
    assert_eq!(sorted(ids(&apps)), sorted(vec![app_a, app_b]));

    let (_, all_access) = mint(&t.fx.db, t.fx.user.id, Reach::all_access()).await;
    let (_, apps) = get_as(&all_access, "/apps/mine").await;
    assert_eq!(
        sorted(ids(&apps)),
        sorted(vec![app_a, app_b]),
        "an all-access token lists what its owner's session lists"
    );

    let (_, bound) = mint(
        &t.fx.db,
        t.fx.user.id,
        Reach::granted(vec![workspace_grant(
            t.fx.org_id,
            t.a1,
            RoleCeiling::Viewer,
        )]),
    )
    .await;
    let (status, apps) = get_as(&bound, "/apps/mine").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ids(&apps), vec![app_a], "org B's app is not listed");
}

/// Routes that answer from the user's raw memberships and have no org or
/// workspace in their path to check a grant against.
const MEMBERSHIP_KEYED: &[(&str, &str)] = &[
    ("GET", "/chat/channels"),
    ("POST", "/chat/channels"),
    ("GET", "/work"),
    ("GET", "/notifications"),
    ("POST", "/notifications/read-all"),
    ("GET", "/notifications/vapid-public-key"),
    ("GET", "/invitations/mine"),
    ("POST", "/invitations/not-a-real-token/accept"),
    ("GET", "/airhouse/me/connection"),
    ("GET", "/oltp/me/connection"),
];

/// The ones that answer 200 on an empty account — what "unchanged" is asserted
/// against for a credential the guard must not touch.
const READS: &[&str] = &["/chat/channels", "/notifications", "/invitations/mine"];

#[tokio::test]
async fn a_grant_bound_token_is_refused_on_routes_that_answer_from_raw_membership() {
    let t = two_orgs().await;
    // However much it was granted: the refusal is about the route.
    for (what, reach) in [
        (
            "one workspace",
            Reach::granted(vec![workspace_grant(t.fx.org_id, t.a1, RoleCeiling::Owner)]),
        ),
        (
            "both orgs, org-wide, uncapped",
            Reach::granted(vec![
                org_grant(t.fx.org_id, RoleCeiling::Owner),
                org_grant(t.org_b, RoleCeiling::Owner),
            ]),
        ),
    ] {
        let (_, secret) = mint(&t.fx.db, t.fx.user.id, reach).await;
        let bearer = format!("Bearer {secret}");
        for (method, uri) in MEMBERSHIP_KEYED {
            let (status, _) = call(
                flat_api(),
                method,
                uri,
                &[("authorization", &bearer)],
                Some(json!({})),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "a token granted {what}: {method} {uri}"
            );
        }
        // …and it still describes itself: the guard refuses routes, not the token.
        assert_eq!(get_as(&secret, "/auth/token").await.0, StatusCode::OK);
    }
}

#[tokio::test]
async fn a_session_and_an_all_access_token_keep_the_flat_routes() {
    let t = two_orgs().await;
    let (_, all_access) = mint(&t.fx.db, t.fx.user.id, Reach::all_access()).await;
    for uri in READS {
        assert_eq!(
            get_in_session(&t.fx, uri).await.0,
            StatusCode::OK,
            "a session on {uri}"
        );
        assert_eq!(
            get_as(&all_access, uri).await.0,
            StatusCode::OK,
            "an all-access token on {uri}"
        );
    }
}
