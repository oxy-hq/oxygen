//! `/api/admin/standing-tokens`: every personal token that carries `platform`
//! or `partner` standing, for staff who hold `manage_platform_grants`
//! (API-tokens design, "Standing tokens, for staff"). The capability admits a
//! browser session, and only one whose platform grant covers every org.

use axum::http::StatusCode;
use chrono::{Duration, Utc};
use entity::api_tokens;
use entity::org_members::OrgRole;
use oxy_authz::{PlatformRole, RoleCeiling};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::{Value, json};
use uuid::Uuid;

use super::browser_session::{in_token_session, session_of};
use super::sandbox_agent::{another_session, grant_staff, minted, post_as, staff_with_app};
use super::service_accounts::{create_account, mint_account_token, with_token};
use super::stack::{
    Reach, flat_api, get_as, get_in_session, ids, join_org, make_staff, mint, org_grant,
};
use super::{audit_rows, call, endpoint_key, key_row, legacy_key, pat_row};

const LIST: &str = "/admin/standing-tokens";

fn revoke_uri(id: impl std::fmt::Display) -> String {
    format!("{LIST}/{id}/revoke")
}

/// An all-access token carrying the standings given — the shape `oxyc login`
/// mints, and the one no org's inventory lists.
fn all_access(platform: bool, partner: bool) -> Reach {
    Reach {
        all_access: true,
        platform,
        partner,
        grants: Vec::new(),
    }
}

/// A token carrying staff standing, bound to a grant in `org_id`.
fn bound_to(org_id: Uuid) -> Reach {
    Reach {
        all_access: false,
        platform: true,
        partner: false,
        grants: vec![org_grant(org_id, RoleCeiling::Member)],
    }
}

/// Make a token `hours` older, so "newest first" does not rest on two inserts
/// landing in different microseconds.
async fn age(db: &DatabaseConnection, id: Uuid, hours: i64) {
    let mut row: api_tokens::ActiveModel = pat_row(db, id).await.into();
    row.created_at = ActiveValue::Set((Utc::now() - Duration::hours(hours)).fixed_offset());
    row.update(db).await.expect("age the token");
}

async fn list_as(cookie: &str) -> (StatusCode, Value) {
    call(flat_api(), "GET", LIST, &[("cookie", cookie)], None).await
}

#[tokio::test]
async fn the_list_holds_personal_tokens_with_a_standing_and_no_other_credential() {
    let (fx, app) = staff_with_app().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Owner).await;
    let (engineer, _) = another_session(&fx.db, "engineer").await;

    let (platform, _) = mint(&fx.db, engineer.id, all_access(true, false)).await;
    age(&fx.db, platform, 2).await;
    let (partner, _) = mint(&fx.db, engineer.id, all_access(false, true)).await;
    age(&fx.db, partner, 1).await;

    // Everything else a person or an org can hold, none of it a standing token.
    let (ordinary, _) = mint(&fx.db, engineer.id, Reach::all_access()).await;
    let (sandbox_agent, _) = minted(&fx, &[app.id]).await;
    let account = create_account(&fx, "deployer", "member").await;
    let (account_token, _) = mint_account_token(&fx, account, json!({ "name": "t" })).await;
    // Legacy keys store both standings always: one the endpoint mirrored at
    // mint, one mirrored when it was first used.
    let (endpoint, _) = endpoint_key(&fx, None).await;
    let (lazy, lazy_key) = legacy_key(&fx, None).await;
    assert_eq!(get_as(&lazy_key, "/auth/token").await.0, StatusCode::OK);

    let (status, list) = get_in_session(&fx, LIST).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        ids(&list["tokens"]),
        vec![partner, platform],
        "only the two standing tokens, newest first: {list}"
    );
    let tokens = list["tokens"].as_array().expect("tokens");
    assert_eq!(
        (&tokens[0]["platform"], &tokens[0]["partner"]),
        (&json!(false), &json!(true))
    );
    assert_eq!(
        (&tokens[1]["platform"], &tokens[1]["partner"]),
        (&json!(true), &json!(false))
    );
    for token in tokens {
        assert_eq!(token["kind"], "personal");
        assert_eq!(token["all_access"], json!(true));
        assert_eq!(token["status"], "active");
        assert_eq!(token["owner"]["id"], json!(engineer.id));
        assert_eq!(
            token["owner"]["label"],
            json!(engineer.email),
            "the owner's email"
        );
        assert!(token.get("secret").is_none() && token.get("token_hash").is_none());
    }

    // The revoke reaches standing tokens only; anything else is not found.
    for (what, id) in [
        ("an ordinary personal token", ordinary),
        ("a sandbox agent token", sandbox_agent),
        ("a service-account token", account_token),
        ("a legacy key the endpoint minted", endpoint),
        ("a legacy key mirrored on use", lazy),
    ] {
        let (status, body) = post_as(&fx.cookie, &revoke_uri(id), json!({})).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{what}: {body}");
        assert_eq!(body, json!({ "error": "not found" }), "{what}");
        assert!(pat_row(&fx.db, id).await.revoked_at.is_none(), "{what}");
    }
    assert!(key_row(&fx.db, endpoint).await.is_active);
    assert!(key_row(&fx.db, lazy).await.is_active);
    let (status, body) = post_as(&fx.cookie, &revoke_uri("not-a-uuid"), json!({})).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body, json!({ "error": "not found" }));
    assert!(audit_rows(&fx.db, "token.revoked").await.is_empty());
}

/// End a token as an `oxyc login` ends its owner's previous one, or by expiry.
async fn end(db: &DatabaseConnection, id: Uuid, expired: bool) {
    let mut row: api_tokens::ActiveModel = pat_row(db, id).await.into();
    let past = (Utc::now() - Duration::minutes(1)).fixed_offset();
    if expired {
        row.expires_at = ActiveValue::Set(Some(past));
    } else {
        row.revoked_at = ActiveValue::Set(Some(past));
    }
    row.update(db).await.expect("end the token");
}

/// Each `oxyc login` retires its owner's previous token, so ended rows pile up
/// faster than working ones. A token that still works must stay on the list
/// however many newer ended ones there are: it is the row the list is for.
#[tokio::test]
async fn a_token_that_still_works_is_never_pushed_off_the_list_by_newer_ended_ones() {
    let (fx, _) = staff_with_app().await;
    let (engineer, _) = another_session(&fx.db, "engineer").await;

    // The oldest row by far, and the only one that still works.
    let (working, _) = mint(&fx.db, engineer.id, all_access(true, false)).await;
    age(&fx.db, working, 90 * 24).await;
    let mut ended = Vec::new();
    for hours in [4, 3, 2, 1] {
        let (id, _) = mint(&fx.db, engineer.id, all_access(true, false)).await;
        age(&fx.db, id, hours).await;
        end(&fx.db, id, hours % 2 == 0).await;
        ended.push(id);
    }

    // Room for three: the one that works, then the two newest ended ones.
    let listed = oxy_auth::token::standing::list(&fx.db, 3)
        .await
        .expect("the list");
    let listed: Vec<Uuid> = listed.iter().map(|t| t.id).collect();
    assert_eq!(listed, vec![ended[3], ended[2], working], "{listed:?}");

    // With room for one, it is the one that works, not the newest row.
    let only = oxy_auth::token::standing::list(&fx.db, 1)
        .await
        .expect("the list");
    assert_eq!(only.iter().map(|t| t.id).collect::<Vec<_>>(), vec![working]);

    // And the page's own listing holds all five, newest first.
    let (status, list) = get_in_session(&fx, LIST).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(
        ids(&list["tokens"]),
        vec![ended[3], ended[2], ended[1], ended[0], working]
    );
}

/// A standing token is a credential for the whole deployment, so a grant
/// bounded to some orgs holds none of them — not even one whose only grant is
/// in an org it reaches. The refusal is decided from the caller alone.
#[tokio::test]
async fn a_bounded_grant_is_refused_whatever_it_names_and_the_tokens_stay_live() {
    let (fx, _) = staff_with_app().await;
    let (engineer, _) = another_session(&fx.db, "engineer").await;
    let (here, _) = mint(&fx.db, engineer.id, bound_to(fx.org_id)).await;
    let (everywhere, secret) = mint(&fx.db, engineer.id, all_access(true, true)).await;

    // A Global Admin — the capability is held — bounded to the fixture's org.
    let (bounded, bounded_cookie) = another_session(&fx.db, "bounded").await;
    grant_staff(
        &fx.db,
        bounded.email.as_deref().unwrap(),
        PlatformRole::GlobalAdmin,
        &[fx.org_id],
    )
    .await;

    let (status, refusal) = list_as(&bounded_cookie).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refusal}");
    assert_eq!(refusal["code"], "unbounded_grant_required", "{refusal}");
    assert!(refusal.get("tokens").is_none(), "{refusal}");

    // A token in its own org, one outside every org, an id that names no
    // token and an id that is not one: the same answer, so it tells nothing
    // of what exists.
    for id in [
        here.to_string(),
        everywhere.to_string(),
        Uuid::new_v4().to_string(),
        "not-a-uuid".to_string(),
    ] {
        let (status, body) = post_as(&bounded_cookie, &revoke_uri(&id), json!({})).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{id}: {body}");
        assert_eq!(body, refusal, "{id}: one answer whatever is named");
    }
    for id in [here, everywhere] {
        assert!(pat_row(&fx.db, id).await.revoked_at.is_none());
    }
    assert!(audit_rows(&fx.db, "token.revoked").await.is_empty());
    assert_eq!(
        get_as(&secret, "/auth/token").await.0,
        StatusCode::OK,
        "still live: its next request is admitted"
    );

    // The fixture's grant covers every org: it sees both, and may end either.
    let (status, list) = get_in_session(&fx, LIST).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    let mut seen = ids(&list["tokens"]);
    seen.sort();
    let mut all = vec![here, everywhere];
    all.sort();
    assert_eq!(seen, all, "{list}");
    let (status, revoked) = post_as(&fx.cookie, &revoke_uri(here), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["status"], "revoked");
    assert_eq!(pat_row(&fx.db, here).await.revoked_by, Some(fx.user.id));
}

#[tokio::test]
async fn a_staff_revoke_ends_the_token_at_once_and_is_audited_once_as_the_staff_member() {
    let (fx, _) = staff_with_app().await;
    // Another staff member's `oxyc login` token: all-access, staff standing.
    let (engineer, _) = another_session(&fx.db, "engineer").await;
    make_staff(&fx.db, engineer.email.as_deref().unwrap()).await;
    join_org(&fx.db, fx.org_id, engineer.id, OrgRole::Member).await;
    let (id, secret) = mint(&fx.db, engineer.id, all_access(true, false)).await;
    let (status, me) = get_as(&secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK, "admitted, and now cached: {me}");

    let (status, revoked) = post_as(&fx.cookie, &revoke_uri(id), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{revoked}");
    assert_eq!(revoked["id"], json!(id));
    assert_eq!(revoked["status"], "revoked");
    assert!(revoked["revoked_at"].is_string(), "{revoked}");
    assert_eq!(revoked["owner"]["label"], json!(engineer.email));

    // The credential cache is not cleared here: the revoke invalidated it, so
    // the very next request on this pod is refused, not 30 seconds later.
    assert_eq!(
        get_as(&secret, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED,
        "refused at once"
    );

    let row = pat_row(&fx.db, id).await;
    assert_eq!(row.revoke_reason.as_deref(), Some("staff"));
    assert_eq!(row.revoked_by, Some(fx.user.id));
    let audit = audit_rows(&fx.db, "token.revoked").await;
    assert_eq!(audit.len(), 1, "one row: the one org the token reached");
    assert_eq!(audit[0].org_id, Some(fx.org_id));
    assert_eq!(
        audit[0].actor_user_id,
        Some(fx.user.id),
        "audited as the staff member, not the owner"
    );
    assert_eq!(audit[0].metadata["reason"], "staff");
    assert_eq!(audit[0].metadata["token_id"], json!(id));

    // Idempotent: nothing changes and nothing is recorded the second time.
    let (status, again) = post_as(&fx.cookie, &revoke_uri(id), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["status"], "revoked");
    assert_eq!(again["revoked_at"], revoked["revoked_at"]);
    assert_eq!(pat_row(&fx.db, id).await.revoked_at, row.revoked_at);
    assert_eq!(audit_rows(&fx.db, "token.revoked").await.len(), 1);

    // An ended token stays on the list.
    let (_, list) = get_in_session(&fx, LIST).await;
    assert_eq!(ids(&list["tokens"]), vec![id], "{list}");
    assert_eq!(list["tokens"][0]["status"], "revoked");
}

#[tokio::test]
async fn only_a_browser_session_holding_the_capability_may_call() {
    let (fx, app) = staff_with_app().await;
    let (target, _) = mint(&fx.db, fx.user.id, all_access(true, false)).await;
    let revoke = revoke_uri(target);

    // No credential at all.
    let (status, _) = call(flat_api(), "GET", LIST, &[], None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // An App Operator is staff and holds `manage_apps`, not
    // `manage_platform_grants`: the capability guard's bare 403.
    let (operator, operator_cookie) = another_session(&fx.db, "operator").await;
    grant_staff(
        &fx.db,
        operator.email.as_deref().unwrap(),
        PlatformRole::AppOperator,
        &[fx.org_id],
    )
    .await;
    // Someone with no standing does not get past the console's door.
    let (_, outsider_cookie) = another_session(&fx.db, "outsider").await;
    for cookie in [&operator_cookie, &outsider_cookie] {
        for (status, body) in [
            list_as(cookie).await,
            post_as(cookie, &revoke, json!({})).await,
        ] {
            assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
            assert!(
                body.get("code").is_none(),
                "the guard's 403, not the session rule's: {body}"
            );
        }
    }

    // A credential cannot manage credentials. The staff member's own
    // all-access token carries their standing, so it passes the capability
    // guard — and is refused as `/api/user/tokens` refuses it. A legacy key
    // is refused the same way there, and so here: these routes are new, so
    // that takes nothing from a key that exists.
    let (_, pat) = mint(&fx.db, fx.user.id, all_access(true, false)).await;
    let (_, legacy) = legacy_key(&fx, None).await;
    for (what, secret) in [("a personal token", &pat), ("a legacy key", &legacy)] {
        for (method, uri) in [("GET", LIST), ("POST", revoke.as_str())] {
            let (status, body) = with_token(secret, method, uri, None).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{what} {method}: {body}");
            assert_eq!(body["code"], "session_required", "{what} {method}: {body}");
        }
    }

    // Nor can the browser session a token opens (`oxyc login-link`): that
    // session is the token, standing and all, and is refused as the token is.
    let opened = session_of(&pat).await;
    for (method, uri) in [("GET", LIST), ("POST", revoke.as_str())] {
        let (status, body) = in_token_session(&opened, method, uri).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method}: {body}");
        assert_eq!(body["code"], "session_required", "{method}: {body}");
    }

    // A sandbox agent token is outside its route allow-list: not found.
    let (_, agent) = minted(&fx, &[app.id]).await;
    let (status, body) = with_token(&agent, "GET", LIST, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");

    assert!(pat_row(&fx.db, target).await.revoked_at.is_none());
    assert!(audit_rows(&fx.db, "token.revoked").await.is_empty());

    // The session those credentials belong to is admitted.
    let (status, list) = get_in_session(&fx, LIST).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["tokens"].as_array().map(Vec::len), Some(2), "{list}");
}
