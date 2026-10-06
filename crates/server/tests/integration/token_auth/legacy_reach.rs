//! The Phase 2 legacy-key regression (design §3.5): after grants, ceilings,
//! standing flags, credential-bound assume sessions and the flat-route guard,
//! a legacy key reaches **exactly what it reached before** — every workspace
//! and org of its owner at the owner's full role, staff standing, the flat
//! routes, on `/api`, `/external/api` and the custom-app path.
//!
//! Both shapes of legacy credential are covered: an `oxy_<hex>` key written
//! before the migration, and an `oxy_pat_` minted by the legacy
//! `/api/{workspace_id}/api-keys` endpoint. Assume-session inheritance is in
//! `assume_binding`.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use oxy_app::server::api::custom_apps_auth::user_can_access_app;
use serde_json::json;
use uuid::Uuid;

use super::stack::{
    custom_app_caller, get_as, ids, join_org, make_staff, published_app, workspace_api,
    workspace_external,
};
use super::{Fixture, call, endpoint_key, fixture, legacy_key, seed_org, seed_workspace_in};

pub(crate) struct World {
    pub fx: Fixture,
    pub org_b: Uuid,
    /// Two workspaces in the fixture's org and one in org B.
    pub workspaces: [Uuid; 3],
}

/// A staff member who owns two orgs.
pub(crate) async fn world() -> World {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let a2 = seed_workspace_in(&fx.db, fx.org_id).await;
    let org_b = seed_org(&fx.db).await;
    join_org(&fx.db, org_b, fx.user.id, OrgRole::Owner).await;
    let b1 = seed_workspace_in(&fx.db, org_b).await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    World {
        workspaces: [fx.workspace_id, a2, b1],
        org_b,
        fx,
    }
}

pub(crate) async fn assert_full_reach(w: &World, what: &str, secret: &str) {
    let bearer = format!("Bearer {secret}");

    // Every workspace of every org, at the owner's own role — read, write and
    // administer — on `/api` and on `/external/api`.
    for workspace in w.workspaces {
        for (method, route) in [("GET", "read"), ("POST", "write"), ("POST", "manage")] {
            let uri = format!("/{workspace}/{route}");
            let (status, _) = call(
                workspace_api(),
                method,
                &uri,
                &[("authorization", &bearer)],
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{what}: /api{uri}");
            let (status, _) = call(
                workspace_external(),
                method,
                &uri,
                &[("x-api-key", secret)],
                None,
            )
            .await;
            assert_eq!(status, StatusCode::NO_CONTENT, "{what}: /external/api{uri}");
        }
    }

    // Discovery lists everything.
    let (status, orgs) = get_as(secret, "/orgs").await;
    assert_eq!(status, StatusCode::OK, "{what}");
    let mut listed = ids(&orgs);
    listed.sort();
    let mut both = vec![w.fx.org_id, w.org_b];
    both.sort();
    assert_eq!(listed, both, "{what}: both orgs");
    let (_, workspaces) = get_as(secret, &format!("/orgs/{}/workspaces", w.fx.org_id)).await;
    assert_eq!(
        ids(&workspaces).len(),
        2,
        "{what}: both workspaces of org A"
    );
    let (status, org) = get_as(secret, &format!("/orgs/{}", w.org_b)).await;
    assert_eq!(status, StatusCode::OK, "{what}");
    assert_eq!(org["role"], "owner", "{what}: no ceiling on the org role");

    // The flat routes the grant guard closes to a grant-bound token.
    for uri in ["/chat/channels", "/notifications", "/invitations/mine"] {
        assert_eq!(get_as(secret, uri).await.0, StatusCode::OK, "{what}: {uri}");
    }

    // Staff standing.
    assert_eq!(
        get_as(secret, "/admin/orgs-meta").await.0,
        StatusCode::OK,
        "{what}: platform standing rides a legacy key"
    );

    // It describes itself as all-access with both standings.
    let (status, me) = get_as(secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK, "{what}");
    assert_eq!(me["kind"], "legacy_key", "{what}");
    assert_eq!(
        (&me["all_access"], &me["platform"], &me["partner"]),
        (&json!(true), &json!(true), &json!(true)),
        "{what}"
    );
    assert_eq!(me["grants"], json!([]), "{what}");
}

#[tokio::test]
async fn a_legacy_key_keeps_its_full_reach() {
    let w = world().await;
    let (_, key) = legacy_key(&w.fx, None).await;
    assert_full_reach(&w, "an oxy_<hex> key", &key).await;
}

#[tokio::test]
async fn a_key_minted_by_the_legacy_endpoint_keeps_its_full_reach() {
    let w = world().await;
    let (_, key) = endpoint_key(&w.fx, None).await;
    assert_full_reach(&w, "a key the legacy endpoint minted", &key).await;
}

#[tokio::test]
async fn a_legacy_key_reaches_every_custom_app_and_lists_them() {
    let w = world().await;
    let mut apps = Vec::new();
    for (org, workspace) in [
        (w.fx.org_id, w.workspaces[0]),
        (w.fx.org_id, w.workspaces[1]),
        (w.org_b, w.workspaces[2]),
    ] {
        apps.push(published_app(&w.fx.db, org, workspace).await);
    }

    for (what, secret) in [
        ("an oxy_<hex> key", legacy_key(&w.fx, None).await.1),
        (
            "a legacy-endpoint oxy_pat_",
            endpoint_key(&w.fx, None).await.1,
        ),
    ] {
        let caller = custom_app_caller(&w.fx, &secret).await;
        for app in &apps {
            assert!(
                user_can_access_app(&w.fx.db, &caller, app).await.unwrap(),
                "{what}: the custom-app path reaches every app"
            );
        }
        let (status, mine) = get_as(&secret, "/apps/mine").await;
        assert_eq!(status, StatusCode::OK, "{what}: {mine}");
        assert_eq!(ids(&mine).len(), apps.len(), "{what}: /apps/mine");
    }
}
