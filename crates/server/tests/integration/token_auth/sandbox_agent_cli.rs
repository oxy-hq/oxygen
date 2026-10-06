//! Minting a sandbox agent token from `oxyc`: the PKCE loopback exchange of
//! `oxyc login`, carrying a `mint` (sandbox agent credential design §5).
//!
//! The browser approves under its session, where who may mint is checked; the
//! CLI, holding no credential, redeems the code for an `oxy_sbx_`. An
//! `authorize` with no `mint` is `oxyc login`, covered in `cli_login`.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::{api_tokens, app_admins, org_token_policies};
use oxy_auth::token::StoredKind;
use oxy_auth::token::cli_login::challenge_of;
use sea_orm::{ActiveModelTrait, ActiveValue::Set, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::{Value, json};
use uuid::Uuid;

use super::sandbox_agent::{another_session, assert_refused, post_as, staff_with_app};
use super::stack::{Reach, flat_api, get_as, mint};
use super::{audit_rows, call};

/// RFC 7636 appendix B's verifier.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

async fn authorize(cookie: &str, hostname: &str, mint: Option<Value>) -> (StatusCode, Value) {
    let mut body = json!({ "code_challenge": challenge_of(VERIFIER), "hostname": hostname });
    if let Some(mint) = mint {
        body["mint"] = mint;
    }
    post_as(cookie, "/auth/cli/authorize", body).await
}

async fn exchange(code: &str) -> (StatusCode, Value) {
    call(
        flat_api(),
        "POST",
        "/auth/cli/exchange",
        &[],
        Some(json!({ "code": code, "code_verifier": VERIFIER })),
    )
    .await
}

fn cli_mint(apps: &[Uuid]) -> Value {
    json!({ "kind": "sandbox_agent", "apps": apps, "expires_in_hours": 2, "name": "nightly agent" })
}

#[tokio::test]
async fn the_cli_exchange_mints_what_the_session_approved() {
    let (fx, app) = staff_with_app().await;
    // An earlier `oxyc login` from the same host, which must survive.
    let (_, login) = authorize(&fx.cookie, "build-box", None).await;
    let (status, login) = exchange(login["code"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{login}");
    let login_secret = login["secret"].as_str().unwrap().to_string();
    assert!(login_secret.starts_with("oxy_pat_"));

    let (status, body) = authorize(&fx.cookie, "build-box", Some(cli_mint(&[app.id]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let code = body["code"].as_str().expect("a code").to_string();

    let (status, minted) = exchange(&code).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let secret = minted["secret"].as_str().expect("the secret");
    assert!(secret.starts_with("oxy_sbx_"), "{secret}");
    let token = &minted["token"];
    assert_eq!(token["kind"], "sandbox_agent");
    assert_eq!(token["name"], "nightly agent");
    assert_eq!(token["source"], "oxyc");
    assert_eq!(token["all_access"], false);
    assert_eq!(token["grants"][0]["app_id"], json!(app.id));
    let expires = DateTime::parse_from_rfc3339(token["expires_at"].as_str().unwrap())
        .unwrap()
        .with_timezone(&Utc);
    let lifetime = expires - Utc::now();
    assert!(
        lifetime > Duration::minutes(119) && lifetime <= Duration::hours(2),
        "2 hours, got {lifetime}"
    );

    // The login's replacement rule is the login's: nothing was retired.
    assert_eq!(get_as(&login_secret, "/auth/token").await.0, StatusCode::OK);
    assert!(audit_rows(&fx.db, "token.revoked").await.is_empty());

    // Audited as the minter, in the app's org, naming the host.
    let created = audit_rows(&fx.db, "token.created").await;
    let row = created
        .iter()
        .find(|r| r.metadata["token_kind"] == "sandbox_agent")
        .expect("the mint's row");
    assert_eq!(row.org_id, Some(fx.org_id));
    assert_eq!(row.actor_user_id, Some(fx.user.id));
    assert_eq!(row.metadata["hostname"], "build-box");

    // Single use, like any code.
    let (status, again) = exchange(&code).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(again["code"], "invalid_code");
}

#[tokio::test]
async fn the_cli_mint_is_checked_under_the_session_before_any_code_is_issued() {
    let (fx, app) = staff_with_app().await;
    let (_, stranger_cookie) = another_session(&fx.db, "stranger").await;

    // Someone who may not mint for the app: no code.
    let refused = authorize(&stranger_cookie, "host", Some(cli_mint(&[app.id]))).await;
    assert_eq!(refused.1["app_id"], json!(app.id));
    assert_refused(
        refused,
        StatusCode::NOT_FOUND,
        "app_not_found",
        "a stranger",
    );
    let issued = entity::cli_auth_codes::Entity::find()
        .all(&fx.db)
        .await
        .unwrap();
    assert!(issued.is_empty(), "a refused mint issues no code");

    // A mint out of range: no code.
    let mut too_long = cli_mint(&[app.id]);
    too_long["expires_in_hours"] = json!(1000);
    let refused = authorize(&fx.cookie, "host", Some(too_long)).await;
    assert_refused(
        refused,
        StatusCode::BAD_REQUEST,
        "invalid_sandbox_token",
        "a lifetime past the limit",
    );

    // A mint of any other kind is not a login.
    let refused = authorize(&fx.cookie, "host", Some(json!({ "kind": "personal" }))).await;
    assert_refused(
        refused,
        StatusCode::BAD_REQUEST,
        "invalid_sandbox_token",
        "a personal mint",
    );

    // A token cannot ask, mint or no mint.
    let (_, pat) = mint(&fx.db, fx.user.id, Reach::all_access().with_platform()).await;
    let bearer = format!("Bearer {pat}");
    let mut body = json!({ "code_challenge": challenge_of(VERIFIER), "hostname": "host" });
    body["mint"] = cli_mint(&[app.id]);
    let refused = call(
        flat_api(),
        "POST",
        "/auth/cli/authorize",
        &[("authorization", &bearer)],
        Some(body),
    )
    .await;
    assert_refused(refused, StatusCode::FORBIDDEN, "session_required", "a PAT");

    // The name is optional from the CLI: it defaults to the host.
    let mut unnamed = cli_mint(&[app.id]);
    unnamed.as_object_mut().unwrap().remove("name");
    let (status, body) = authorize(&fx.cookie, "ops-laptop", Some(unnamed)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, minted) = exchange(body["code"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(minted["token"]["name"], "sandbox agent on ops-laptop");
}

/// An org's lifetime cap is asked at the approval, where the browser can say
/// why. Asked only at the exchange, the page said "approved" and the CLI was
/// left with `invalid_code`.
#[tokio::test]
async fn an_approval_past_an_orgs_lifetime_cap_is_refused_before_any_code_is_issued() {
    let (fx, app) = staff_with_app().await;
    org_token_policies::ActiveModel {
        org_id: Set(fx.org_id),
        max_lifetime_days: Set(Some(1)),
        allow_all_access_tokens: Set(true),
        require_environment_on_trust_policies: Set(false),
        updated_by: Set(None),
        updated_at: Set(Utc::now().into()),
    }
    .insert(&fx.db)
    .await
    .expect("cap the org's tokens at one day");

    let mut two_days = cli_mint(&[app.id]);
    two_days["expires_in_hours"] = json!(48);
    let refused = authorize(&fx.cookie, "host", Some(two_days)).await;
    assert_eq!(refused.1["max_lifetime_days"], 1, "{}", refused.1);
    assert_refused(
        refused,
        StatusCode::BAD_REQUEST,
        "exceeds_policy",
        "two days against a one-day cap",
    );
    let issued = entity::cli_auth_codes::Entity::find()
        .all(&fx.db)
        .await
        .unwrap();
    assert!(issued.is_empty(), "a refused mint issues no code");

    // The same approval within the cap goes through.
    let (status, approved) = authorize(&fx.cookie, "host", Some(cli_mint(&[app.id]))).await;
    assert_eq!(status, StatusCode::OK, "two hours fits: {approved}");
}

#[tokio::test]
async fn an_approval_the_minter_can_no_longer_back_mints_nothing() {
    let (fx, app) = staff_with_app().await;
    let (status, body) = authorize(&fx.cookie, "host", Some(cli_mint(&[app.id]))).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Between the approval and the exchange, the minter's grant is taken away.
    app_admins::Entity::delete_many()
        .filter(app_admins::Column::Email.eq(fx.user.email.clone().unwrap()))
        .exec(&fx.db)
        .await
        .expect("take the grant away");

    let (status, refused) = exchange(body["code"].as_str().unwrap()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert_eq!(refused["code"], "invalid_code");
    assert!(refused.get("secret").is_none());
    let sandbox_tokens = api_tokens::Entity::find()
        .filter(api_tokens::Column::Kind.eq(StoredKind::SandboxAgent.as_str()))
        .all(&fx.db)
        .await
        .unwrap();
    assert!(sandbox_tokens.is_empty(), "nothing was minted");
}

#[tokio::test]
async fn a_mint_code_is_stored_where_a_login_lookup_cannot_find_it() {
    // The property a binary one release back depends on: it finds a code by
    // the SHA-256 of the code alone and mints an all-access login token for
    // whatever it finds. A mint code must not be findable that way.
    let (fx, app) = staff_with_app().await;
    let (_, body) = authorize(&fx.cookie, "host", Some(cli_mint(&[app.id]))).await;
    let code = body["code"].as_str().expect("a code");

    // SHA-256 of the code, which is exactly how a login code is keyed.
    let login_hash = oxy_auth::token::hash_token(code);
    let by_login_hash = entity::cli_auth_codes::Entity::find_by_id(login_hash)
        .one(&fx.db)
        .await
        .unwrap();
    assert!(by_login_hash.is_none(), "a mint code under the login hash");

    let stored = entity::cli_auth_codes::Entity::find()
        .all(&fx.db)
        .await
        .unwrap();
    assert_eq!(stored.len(), 1);
    let mint = stored[0].mint.as_ref().expect("the approved mint");
    assert_eq!(mint["kind"], "sandbox_agent");
    assert_eq!(mint["apps"], json!([app.id]));
    assert_eq!(mint["expires_in_hours"], 2);
}
