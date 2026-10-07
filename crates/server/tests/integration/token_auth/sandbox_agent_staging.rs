//! The **staging option** of a sandbox agent token (`oxy_sbx_`): minting with
//! it, what the stored token is admitted as, and what the token routes say
//! of it (sandbox agent credential design, "Staging option").
//!
//! A mint that says `"staging": true` stores one `app_staging` grant beside
//! each app's `app_sandbox` one. A mint that does not is what it always was.
//! The grant is honoured on a sandbox agent token alone, and only beside its
//! twin: a row written by hand anywhere else refuses the token it is on.

use axum::http::StatusCode;
use entity::org_members::OrgRole;
use entity::{api_token_grants, api_tokens, app_admins};
use oxy_auth::token::cli_login::challenge_of;
use oxy_auth::token::{TokenFormat, store};
use oxy_authz::PlatformRole;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::sandbox_agent::{
    admitted, another_session, assert_refused, grant_staff, grants_of, minted, post_as,
    staff_with_app,
};
use super::stack::{Reach, flat_api, get_as, get_in_session, join_org, mint, published_app};
use super::{Fixture, audit_rows, call, seed_org, seed_workspace_in};

/// RFC 7636 appendix B's verifier.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

fn staging_body(apps: &[Uuid], staging: Value) -> Value {
    json!({ "name": "stager", "kind": "sandbox_agent", "apps": apps, "staging": staging })
}

/// Mint with `staging: true` under the fixture's session: `(id, secret, body)`.
async fn minted_with_staging(fx: &Fixture, apps: &[Uuid]) -> (Uuid, String, Value) {
    let body = staging_body(apps, json!(true));
    let (status, body) = post_as(&fx.cookie, "/user/tokens", body).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let id = Uuid::parse_str(body["token"]["id"].as_str().expect("an id")).unwrap();
    let secret = body["secret"].as_str().expect("the secret").to_string();
    (id, secret, body)
}

fn kinds_of(grants: &[api_token_grants::Model]) -> Vec<&str> {
    let mut kinds: Vec<&str> = grants.iter().map(|g| g.kind.as_str()).collect();
    kinds.sort_unstable();
    kinds
}

async fn sandbox_token_count(fx: &Fixture) -> usize {
    api_tokens::Entity::find()
        .filter(api_tokens::Column::Kind.eq("sandbox_agent"))
        .all(&fx.db)
        .await
        .unwrap()
        .len()
}

/// A grant row written by hand under token `token_id`.
async fn hand_written_grant(fx: &Fixture, token_id: Uuid, kind: &str, app_id: Uuid) {
    api_token_grants::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        token_id: ActiveValue::Set(token_id),
        kind: ActiveValue::Set(kind.to_string()),
        org_id: ActiveValue::Set(fx.org_id),
        workspace_id: ActiveValue::Set(None),
        role_ceiling: ActiveValue::Set(None),
        app_id: ActiveValue::Set(Some(app_id)),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        revoked_at: ActiveValue::Set(None),
        revoked_by: ActiveValue::Set(None),
    }
    .insert(&fx.db)
    .await
    .expect("write a grant row by hand");
}

#[tokio::test]
async fn a_mint_with_staging_stores_a_staging_grant_beside_each_apps() {
    let (fx, app) = staff_with_app().await;
    let second = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let (id, secret, body) = minted_with_staging(&fx, &[app.id, second.id]).await;
    assert!(secret.starts_with("oxy_sbx_"), "{secret}");

    // Stored: two rows an app, the kind alone differing.
    let stored = grants_of(&fx.db, id).await;
    assert_eq!(
        kinds_of(&stored),
        vec!["app_sandbox", "app_sandbox", "app_staging", "app_staging"]
    );
    for granted in [app.id, second.id] {
        let of_app: Vec<&api_token_grants::Model> = stored
            .iter()
            .filter(|g| g.app_id == Some(granted))
            .collect();
        assert_eq!(of_app.len(), 2, "{granted}");
        for grant in of_app {
            assert_eq!(grant.org_id, fx.org_id);
            assert_eq!((grant.workspace_id, &grant.role_ceiling), (None, &None));
            assert!(grant.revoked_at.is_none());
        }
    }

    // Told: the token's grants carry the staging rows, with the slugs an
    // `app_sandbox` row carries.
    let grants = body["token"]["grants"].as_array().expect("grants");
    assert_eq!(grants.len(), 4, "{body}");
    let staging: Vec<&Value> = grants
        .iter()
        .filter(|g| g["kind"] == "app_staging")
        .collect();
    assert_eq!(staging.len(), 2, "{body}");
    for grant in staging {
        assert_eq!(grant["org_id"], json!(fx.org_id));
        assert_eq!(grant["app_name"], "Token Test App");
        assert!(grant["app_slug"].as_str().unwrap().starts_with("app-"));
        assert!(grant["org_slug"].as_str().unwrap().starts_with("acme-"));
        assert_eq!(grant["role_ceiling"], Value::Null);
    }

    // Admitted: the same kind of credential, each app marked.
    let credential = admitted(&fx, &secret).await.expect("admitted");
    assert!(credential.is_sandbox_agent());
    assert!(credential.stages_app(app.id) && credential.stages_app(second.id));
    let reach = credential.reach().unwrap().sandbox_agent.unwrap();
    assert!(reach.apps.len() == 2 && reach.apps.iter().all(|a| a.staging));

    // And the token describes itself: `staging` on each of its apps.
    let (status, me) = get_as(&secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK, "{me}");
    let apps = me["apps"].as_array().expect("apps");
    assert_eq!(apps.len(), 2, "{me}");
    assert!(apps.iter().all(|a| a["staging"] == true), "{me}");
    let told = me["grants"].as_array().expect("grants");
    assert_eq!(
        told.iter().filter(|g| g["kind"] == "app_staging").count(),
        2
    );

    // The mint's audit row lists the staging grants among the token's.
    let created = audit_rows(&fx.db, "token.created").await;
    let row = created
        .iter()
        .find(|r| r.metadata["token_kind"] == "sandbox_agent")
        .expect("the mint's row");
    let kinds: Vec<&str> = row.metadata["grants"]
        .as_array()
        .expect("grants")
        .iter()
        .filter_map(|g| g["kind"].as_str())
        .collect();
    assert_eq!(kinds.iter().filter(|k| **k == "app_staging").count(), 2);
}

#[tokio::test]
async fn a_mint_without_staging_is_the_token_it_always_was() {
    let (fx, app) = staff_with_app().await;
    for staging in [None, Some(json!(false)), Some(Value::Null)] {
        let mut body = json!({ "name": "agent", "kind": "sandbox_agent", "apps": [app.id] });
        if let Some(staging) = staging.clone() {
            body["staging"] = staging;
        }
        let (status, body) = post_as(&fx.cookie, "/user/tokens", body).await;
        assert_eq!(status, StatusCode::CREATED, "{staging:?}: {body}");
        let id = Uuid::parse_str(body["token"]["id"].as_str().unwrap()).unwrap();
        let secret = body["secret"].as_str().unwrap().to_string();

        let stored = grants_of(&fx.db, id).await;
        assert_eq!(kinds_of(&stored), vec!["app_sandbox"], "{staging:?}");
        assert_eq!(body["token"]["grants"].as_array().map(Vec::len), Some(1));
        let credential = admitted(&fx, &secret).await.expect("admitted");
        assert!(!credential.stages_app(app.id) && !credential.stages_any_app());
        let (status, me) = get_as(&secret, "/auth/token").await;
        assert_eq!(status, StatusCode::OK, "{me}");
        assert_eq!(me["apps"][0]["staging"], false, "{me}");
    }
}

#[tokio::test]
async fn staging_that_is_not_a_boolean_is_400_and_mints_nothing() {
    let (fx, app) = staff_with_app().await;
    let before = sandbox_token_count(&fx).await;
    for not_a_boolean in [json!("true"), json!(1), json!(["yes"]), json!({})] {
        let body = staging_body(&[app.id], not_a_boolean.clone());
        let refused = post_as(&fx.cookie, "/user/tokens", body).await;
        let why = format!("{not_a_boolean}");
        assert_refused(
            refused,
            StatusCode::BAD_REQUEST,
            "invalid_sandbox_token",
            &why,
        );
    }
    assert_eq!(sandbox_token_count(&fx).await, before);
}

/// Whoever may open an app's staging and mint for it mints with staging;
/// whoever may not open it is told what they are told for an app they may not
/// mint for at all, and nothing is minted.
#[tokio::test]
async fn only_someone_who_may_open_staging_mints_with_it() {
    let (fx, app) = staff_with_app().await;
    let other_org = seed_org(&fx.db).await;
    let other_ws = seed_workspace_in(&fx.db, other_org).await;
    let elsewhere = published_app(&fx.db, other_org, other_ws).await;

    // The app's org owner, who is not staff: staging is Oxy staff's to open.
    let (member, member_cookie) = another_session(&fx.db, "member").await;
    join_org(&fx.db, fx.org_id, member.id, OrgRole::Owner).await;
    // An App Operator whose grant reaches the fixture's org only.
    let (operator, operator_cookie) = another_session(&fx.db, "operator").await;
    let operator_email = operator.email.as_deref().unwrap();
    grant_staff(
        &fx.db,
        operator_email,
        PlatformRole::AppOperator,
        &[fx.org_id],
    )
    .await;

    let before = sandbox_token_count(&fx).await;
    for (cookie, app_id, why) in [
        (&member_cookie, app.id, "an org owner who is not staff"),
        (
            &operator_cookie,
            elsewhere.id,
            "staging of an app outside the grant's scope",
        ),
    ] {
        let refused = post_as(cookie, "/user/tokens", staging_body(&[app_id], json!(true))).await;
        assert_eq!(refused.1["app_id"], json!(app_id), "{why}");
        assert_refused(refused, StatusCode::NOT_FOUND, "app_not_found", why);
    }
    assert_eq!(sandbox_token_count(&fx).await, before, "nothing minted");

    // The operator may open staging of the app inside the grant, and mints.
    let allowed = staging_body(&[app.id], json!(true));
    let (status, body) = post_as(&operator_cookie, "/user/tokens", allowed).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["token"]["grants"].as_array().map(Vec::len), Some(2));

    // Standing is read now: the grant taken away, the next mint is refused.
    app_admins::Entity::delete_many()
        .filter(app_admins::Column::Email.eq(operator_email))
        .exec(&fx.db)
        .await
        .unwrap();
    let again = staging_body(&[app.id], json!(true));
    let refused = post_as(&operator_cookie, "/user/tokens", again).await;
    assert_refused(refused, StatusCode::NOT_FOUND, "app_not_found", "revoked");
}

async fn authorize(cookie: &str, mint: Value) -> (StatusCode, Value) {
    let body = json!({
        "code_challenge": challenge_of(VERIFIER), "hostname": "build-box", "mint": mint,
    });
    post_as(cookie, "/auth/cli/authorize", body).await
}

async fn exchange(code: &str) -> (StatusCode, Value) {
    let body = json!({ "code": code, "code_verifier": VERIFIER });
    call(flat_api(), "POST", "/auth/cli/exchange", &[], Some(body)).await
}

/// The CLI's mint carries `staging` from the approval to the token: checked
/// under the session at `authorize`, and again at `exchange`.
#[tokio::test]
async fn the_cli_mint_carries_staging_and_is_checked_at_both_ends() {
    let (fx, app) = staff_with_app().await;
    let asked = json!({ "kind": "sandbox_agent", "apps": [app.id], "staging": true });

    // Someone who may not open the app's staging: no code.
    let (_, stranger_cookie) = another_session(&fx.db, "stranger").await;
    let refused = authorize(&stranger_cookie, asked.clone()).await;
    assert_refused(refused, StatusCode::NOT_FOUND, "app_not_found", "no code");
    let malformed = json!({ "kind": "sandbox_agent", "apps": [app.id], "staging": "yes" });
    let refused = authorize(&fx.cookie, malformed).await;
    assert_refused(
        refused,
        StatusCode::BAD_REQUEST,
        "invalid_sandbox_token",
        "no code",
    );

    // Approved, and redeemed: the staging grants are the token's.
    let (status, body) = authorize(&fx.cookie, asked.clone()).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, minted) = exchange(body["code"].as_str().expect("a code")).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let secret = minted["secret"].as_str().expect("the secret");
    let id = Uuid::parse_str(minted["token"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(
        kinds_of(&grants_of(&fx.db, id).await),
        vec!["app_sandbox", "app_staging"]
    );
    let (_, me) = get_as(secret, "/auth/token").await;
    assert_eq!(me["apps"][0]["staging"], true, "{me}");

    // Approved, then the approver loses the grant before the code is
    // redeemed: nothing is minted, and the CLI is told what it is told for
    // any code it cannot redeem.
    let (status, body) = authorize(&fx.cookie, asked).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let before = sandbox_token_count(&fx).await;
    app_admins::Entity::delete_many()
        .filter(app_admins::Column::Email.eq(fx.user.email.as_deref().unwrap()))
        .exec(&fx.db)
        .await
        .unwrap();
    let refused = exchange(body["code"].as_str().expect("a code")).await;
    assert_refused(refused, StatusCode::BAD_REQUEST, "invalid_code", "exchange");
    assert_eq!(sandbox_token_count(&fx).await, before);
}

#[tokio::test]
async fn token_options_say_this_server_takes_staging() {
    let (fx, _app) = staff_with_app().await;
    let (status, options) = get_in_session(&fx, "/user/token-options").await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["sandbox_agent"],
        json!({ "default_hours": 8, "max_hours": 168, "max_apps": 5, "staging": true })
    );
}

/// The grant is a sandbox agent token's, and only beside the `app_sandbox`
/// grant of the same app: written by hand anywhere else, it refuses the token
/// it is on — which is also what a binary that does not know the kind does.
#[tokio::test]
async fn a_staging_grant_out_of_place_refuses_the_whole_token() {
    let (fx, app) = staff_with_app().await;
    let second = published_app(&fx.db, fx.org_id, fx.workspace_id).await;

    // On a personal token bound to grants: refused, where it answered before.
    let reach = Reach::granted(vec![super::stack::org_grant(
        fx.org_id,
        oxy_authz::RoleCeiling::Admin,
    )]);
    let (pat, pat_secret) = mint(&fx.db, fx.user.id, reach).await;
    let resolve = || store::resolve(&fx.db, &pat_secret, TokenFormat::Personal);
    assert!(resolve().await.is_ok(), "a grant-bound personal token");
    hand_written_grant(&fx, pat, api_token_grants::KIND_APP_STAGING, app.id).await;
    let refused = resolve().await.expect_err("refused").to_string();
    assert!(refused.contains("Invalid API key"), "{refused}");

    // On a sandbox agent token, for an app it holds no `app_sandbox` grant
    // for: refused whole, its own app included.
    let (id, secret) = minted(&fx, &[app.id]).await;
    assert_eq!(get_as(&secret, "/auth/token").await.0, StatusCode::OK);
    hand_written_grant(&fx, id, api_token_grants::KIND_APP_STAGING, second.id).await;
    let refused = admitted(&fx, &secret).await.expect_err("refused");
    assert!(refused.contains("Invalid API key"), "{refused}");
    assert_eq!(
        get_as(&secret, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED
    );

    // Beside its twin it is the option, as a mint would have written it.
    let (id, secret) = minted(&fx, &[app.id]).await;
    hand_written_grant(&fx, id, api_token_grants::KIND_APP_STAGING, app.id).await;
    let credential = admitted(&fx, &secret).await.expect("admitted");
    assert!(credential.stages_app(app.id));
}
