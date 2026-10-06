//! The personal-token API through the served stack (`/api/user/tokens`,
//! `/api/auth/token`): what the HTTP contract promises, end to end — mounted
//! where the server mounts it, behind the real auth layers.
//!
//! The rule everything else rests on comes first: **a token cannot manage
//! tokens**. Minting, widening, extending and regenerating are session-only.

use axum::http::StatusCode;
use chrono::Utc;
use entity::api_token_grants;
use entity::org_members::OrgRole;
use oxy_authz::RoleCeiling;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::stack::{
    Reach, flat_api, get_as, get_in_session, join_org, mint, workspace_api, workspace_grant,
};
use super::{Fixture, call, endpoint_key, fixture, legacy_key, seed_org, seed_workspace_in};

async fn owner_fixture() -> Fixture {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    fx
}

async fn in_session(
    fx: &Fixture,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    call(flat_api(), method, uri, &[("cookie", &fx.cookie)], body).await
}

async fn with_token(
    secret: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let bearer = format!("Bearer {secret}");
    call(flat_api(), method, uri, &[("authorization", &bearer)], body).await
}

async fn workspace_status(secret: &str, method: &str, uri: &str) -> StatusCode {
    let bearer = format!("Bearer {secret}");
    call(
        workspace_api(),
        method,
        uri,
        &[("authorization", &bearer)],
        None,
    )
    .await
    .0
}

fn grant_json(org: Uuid, workspace: Uuid, ceiling: &str) -> Value {
    json!({ "org_id": org, "workspace_id": workspace, "role_ceiling": ceiling })
}

/// The grants of a `Token` body the org has not revoked.
fn live_grants(token: &Value) -> Vec<&Value> {
    token["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .filter(|g| g["revoked_at"].is_null())
        .collect()
}

#[tokio::test]
async fn a_token_cannot_create_patch_extend_or_regenerate_a_token() {
    let fx = owner_fixture().await;
    let (target, _) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let (_, all_access) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let (_, bound) = mint(
        &fx.db,
        fx.user.id,
        Reach::granted(vec![workspace_grant(
            fx.org_id,
            fx.workspace_id,
            RoleCeiling::Owner,
        )]),
    )
    .await;
    let (_, legacy) = legacy_key(&fx, None).await;

    let attempts = [
        ("POST", "/user/tokens".to_string(), json!({ "name": "x" })),
        (
            "PATCH",
            format!("/user/tokens/{target}"),
            json!({ "name": "y" }),
        ),
        (
            "POST",
            format!("/user/tokens/{target}/extend"),
            json!({ "days": 30 }),
        ),
        (
            "POST",
            format!("/user/tokens/{target}/regenerate"),
            json!({}),
        ),
        ("DELETE", format!("/user/tokens/{target}"), json!({})),
        ("GET", "/user/tokens".to_string(), json!({})),
        (
            "POST",
            "/auth/cli/authorize".to_string(),
            json!({ "code_challenge": "x", "hostname": "h" }),
        ),
    ];
    for (who, secret) in [
        ("an all-access token", &all_access),
        ("a grant-bound token", &bound),
        ("a legacy key", &legacy),
    ] {
        for (method, uri, body) in &attempts {
            let (status, answer) = with_token(secret, method, uri, Some(body.clone())).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{who}: {method} {uri}");
            assert_eq!(
                answer["code"], "session_required",
                "{who}: {method} {uri}: {answer}"
            );
        }
    }
    // Nothing happened to the token they were aimed at.
    let (status, token) = in_session(&fx, "GET", &format!("/user/tokens/{target}"), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(token["status"], "active");
    assert_eq!(token["name"], "phase 2");
}

#[tokio::test]
async fn create_defaults_to_all_access_and_the_secret_is_the_token() {
    let fx = owner_fixture().await;
    let (status, created) = in_session(
        &fx,
        "POST",
        "/user/tokens",
        Some(json!({ "name": "laptop" })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let token = &created["token"];
    assert_eq!(token["all_access"], true);
    assert_eq!(token["platform"], false);
    assert_eq!(token["partner"], false);
    assert_eq!(token["kind"], "personal");
    assert_eq!(token["source"], "ui");
    assert_eq!(token["status"], "active");
    assert_eq!(token["grants"], json!([]));
    assert!(token["expires_at"].is_string(), "90 days unless asked");
    assert_eq!(token["owner"]["id"], json!(fx.user.id));
    let secret = created["secret"].as_str().expect("the secret, once");
    assert!(secret.starts_with("oxy_pat_"));

    let (status, me) = get_as(secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["id"], token["id"]);
    assert!(
        me.get("secret").is_none(),
        "the secret is never shown again"
    );
}

#[tokio::test]
async fn create_refuses_a_standing_or_a_grant_the_session_does_not_hold() {
    let fx = owner_fixture().await;
    let (status, answer) = in_session(
        &fx,
        "POST",
        "/user/tokens",
        Some(json!({ "name": "staff", "platform": true })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(answer["code"], "standing_required");

    // An org the user is not in reads as missing, not as forbidden.
    let foreign = seed_org(&fx.db).await;
    let (status, _) = in_session(
        &fx,
        "POST",
        "/user/tokens",
        Some(json!({ "name": "n", "all_access": false, "grants": [{ "org_id": foreign }] })),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = in_session(
        &fx,
        "POST",
        "/user/tokens",
        Some(json!({ "name": "n", "all_access": false })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a narrowed token names grants"
    );
}

#[tokio::test]
async fn patch_replaces_grants_and_never_resurrects_one_the_org_revoked() {
    let fx = owner_fixture().await;
    let (a, b) = (fx.workspace_id, seed_workspace_in(&fx.db, fx.org_id).await);
    let (status, created) = in_session(
        &fx,
        "POST",
        "/user/tokens",
        Some(json!({
            "name": "ci",
            "all_access": false,
            "grants": [grant_json(fx.org_id, a, "member"), grant_json(fx.org_id, b, "member")],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let id = created["token"]["id"].as_str().unwrap().to_string();
    let secret = created["secret"].as_str().unwrap().to_string();
    assert_eq!(live_grants(&created["token"]).len(), 2);
    assert_eq!(
        workspace_status(&secret, "GET", &format!("/{b}/read")).await,
        StatusCode::NO_CONTENT
    );

    // The org revokes the grant on B (Phase 3 gives it a button; the row is
    // what the token machinery reads).
    let on_b = api_token_grants::Entity::find()
        .filter(api_token_grants::Column::TokenId.eq(Uuid::parse_str(&id).unwrap()))
        .filter(api_token_grants::Column::WorkspaceId.eq(b))
        .one(&fx.db)
        .await
        .unwrap()
        .expect("the grant on B");
    let mut revoked: api_token_grants::ActiveModel = on_b.into();
    revoked.revoked_at = ActiveValue::Set(Some(Utc::now().fixed_offset()));
    revoked.update(&fx.db).await.expect("org revokes the grant");
    oxy_auth::token::cache::clear();
    assert_eq!(
        workspace_status(&secret, "GET", &format!("/{b}/read")).await,
        StatusCode::NOT_FOUND,
        "an org-revoked grant grants nothing"
    );

    // The owner asks for both again, A at a lower ceiling.
    let (status, patched) = in_session(
        &fx,
        "PATCH",
        &format!("/user/tokens/{id}"),
        Some(json!({
            "grants": [grant_json(fx.org_id, a, "viewer"), grant_json(fx.org_id, b, "owner")],
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    let live = live_grants(&patched);
    assert_eq!(live.len(), 1, "B is not granted again: {patched}");
    assert_eq!(live[0]["workspace_id"], json!(a));
    assert_eq!(live[0]["role_ceiling"], "viewer");

    assert_eq!(
        workspace_status(&secret, "GET", &format!("/{b}/read")).await,
        StatusCode::NOT_FOUND,
        "the edit did not resurrect the revoked grant"
    );
    assert_eq!(
        workspace_status(&secret, "GET", &format!("/{a}/read")).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        workspace_status(&secret, "POST", &format!("/{a}/write")).await,
        StatusCode::FORBIDDEN,
        "and A is now capped at viewer"
    );

    // Asking for the revoked target alone leaves nothing to grant.
    let (status, _) = in_session(
        &fx,
        "PATCH",
        &format!("/user/tokens/{id}"),
        Some(json!({ "grants": [grant_json(fx.org_id, b, "owner")] })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn regenerate_keeps_the_token_and_kills_the_old_secret() {
    let fx = owner_fixture().await;
    let (id, old) = mint(
        &fx.db,
        fx.user.id,
        Reach::granted(vec![workspace_grant(
            fx.org_id,
            fx.workspace_id,
            RoleCeiling::Member,
        )]),
    )
    .await;
    assert_eq!(get_as(&old, "/auth/token").await.0, StatusCode::OK);

    let (status, regenerated) =
        in_session(&fx, "POST", &format!("/user/tokens/{id}/regenerate"), None).await;
    assert_eq!(status, StatusCode::OK, "{regenerated}");
    assert_eq!(regenerated["token"]["id"], json!(id), "same token");
    assert_eq!(
        live_grants(&regenerated["token"]).len(),
        1,
        "same grants: {regenerated}"
    );
    let new = regenerated["secret"].as_str().unwrap();
    assert_ne!(new, old);

    assert_eq!(
        get_as(&old, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED,
        "the old secret dies at once"
    );
    let (status, me) = get_as(new, "/auth/token").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["id"], json!(id));
}

#[tokio::test]
async fn revoke_ends_the_token_and_the_list_keeps_showing_it() {
    let fx = owner_fixture().await;
    let (id, secret) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    // A legacy key with its mirror row: it is not a token, so the token list
    // never shows it.
    let (_, key) = legacy_key(&fx, None).await;
    assert_eq!(get_as(&key, "/auth/token").await.0, StatusCode::OK);

    let (status, _) = in_session(&fx, "DELETE", &format!("/user/tokens/{id}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        get_as(&secret, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED
    );

    let (status, list) = get_in_session(&fx, "/user/tokens").await;
    assert_eq!(status, StatusCode::OK);
    let tokens = list["tokens"].as_array().expect("tokens");
    let revoked = tokens
        .iter()
        .find(|t| t["id"] == json!(id))
        .expect("a revoked token is still listed");
    assert_eq!(revoked["status"], "revoked");
    assert!(revoked["revoked_at"].is_string());
    assert!(
        tokens.iter().all(|t| t["kind"] == "personal"),
        "the token list holds personal tokens only, never a legacy key: {list}"
    );

    // Nothing more happens to a revoked token.
    let (status, answer) = in_session(
        &fx,
        "POST",
        &format!("/user/tokens/{id}/extend"),
        Some(json!({ "days": 30 })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(answer["code"], "revoked");
}

#[tokio::test]
async fn the_calling_token_describes_and_revokes_itself() {
    let fx = owner_fixture().await;

    // A session has no calling token.
    for method in ["GET", "DELETE"] {
        let (status, answer) = in_session(&fx, method, "/auth/token", None).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} under a session");
        assert_eq!(answer["code"], "no_token");
    }

    let (id, secret) = mint(&fx.db, fx.user.id, Reach::all_access()).await;
    let (status, me) = get_as(&secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["id"], json!(id));
    assert_eq!(me["kind"], "personal");
    let (status, _) = with_token(&secret, "DELETE", "/auth/token", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        get_as(&secret, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED,
        "it revoked itself"
    );
}

#[tokio::test]
async fn a_legacy_key_is_not_a_token_on_any_token_route() {
    // Legacy API keys and API tokens are separate (decided 2026-10-05). A
    // legacy key is managed through its own routes; on the token routes it is
    // absent from the list and its id reads as nothing at all.
    let fx = owner_fixture().await;
    let (minted, minted_key) = endpoint_key(&fx, None).await;
    let (seeded, seeded_key) = legacy_key(&fx, None).await;
    // Used once, so the seeded key has its `api_tokens` mirror row too.
    assert_eq!(get_as(&seeded_key, "/auth/token").await.0, StatusCode::OK);

    let (status, list) = get_in_session(&fx, "/user/tokens").await;
    assert_eq!(status, StatusCode::OK);
    let listed = list["tokens"].as_array().expect("tokens");
    for id in [minted, seeded] {
        assert!(
            listed.iter().all(|t| t["id"] != json!(id)),
            "a legacy key in the token list: {list}"
        );
        for (method, uri, body) in [
            ("GET", format!("/user/tokens/{id}"), None),
            (
                "PATCH",
                format!("/user/tokens/{id}"),
                Some(json!({ "name": "renamed" })),
            ),
            (
                "POST",
                format!("/user/tokens/{id}/extend"),
                Some(json!({ "days": 30 })),
            ),
            (
                "POST",
                format!("/user/tokens/{id}/regenerate"),
                Some(json!({})),
            ),
            ("GET", format!("/user/tokens/{id}/activity"), None),
            ("DELETE", format!("/user/tokens/{id}"), None),
        ] {
            let (status, answer) = in_session(&fx, method, &uri, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}: {answer}");
        }
    }

    // None of that touched either key: both still work, as legacy keys.
    for key in [&minted_key, &seeded_key] {
        let (status, me) = get_as(key, "/auth/token").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(me["kind"], "legacy_key");
        assert_eq!(me["name"].is_string(), true);
    }
}

#[tokio::test]
async fn a_legacy_key_cannot_revoke_itself_through_the_new_door() {
    let fx = owner_fixture().await;
    let (_, key) = legacy_key(&fx, None).await;
    let (_, endpoint_pat) = endpoint_key(&fx, None).await;

    for (what, secret) in [
        ("an oxy_<hex> key", &key),
        ("a legacy-endpoint key", &endpoint_pat),
    ] {
        let (status, me) = get_as(secret, "/auth/token").await;
        assert_eq!(status, StatusCode::OK, "{what}");
        assert_eq!(me["kind"], "legacy_key", "{what}");
        assert_eq!(me["all_access"], true, "{what}");

        let (status, answer) = with_token(secret, "DELETE", "/auth/token", None).await;
        assert_eq!(status, StatusCode::CONFLICT, "{what}");
        assert_eq!(answer["code"], "legacy_immutable", "{what}");
        assert_eq!(
            get_as(secret, "/auth/token").await.0,
            StatusCode::OK,
            "{what} still works: nothing here ends a legacy key"
        );
    }
}
