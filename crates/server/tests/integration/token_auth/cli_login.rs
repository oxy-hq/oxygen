//! `oxyc login` end to end (design §6): the browser trades the CLI's S256
//! challenge for a one-time code under its session, and the CLI — holding no
//! credential yet — redeems `code + verifier` for an `oxy_pat_`.
//!
//! Every way of failing answers the same 400 `invalid_code`, and any attempt
//! spends the code.

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::org_members::OrgRole;
use entity::{cli_auth_codes, org_token_policies};
use oxy_auth::token::cli_login::challenge_of;
use sea_orm::sea_query::Expr;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};
use serde_json::{Value, json};

use super::stack::{flat_api, get_as, join_org, make_staff};
use super::{Fixture, call, fixture};

/// RFC 7636 appendix B's verifier.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

async fn authorize_with(fx: &Fixture, challenge: &str, hostname: &str) -> (StatusCode, Value) {
    call(
        flat_api(),
        "POST",
        "/auth/cli/authorize",
        &[("cookie", &fx.cookie)],
        Some(json!({ "code_challenge": challenge, "hostname": hostname })),
    )
    .await
}

/// A code for [`VERIFIER`], issued to the fixture's session.
async fn code_for(fx: &Fixture, hostname: &str) -> String {
    let (status, body) = authorize_with(fx, &challenge_of(VERIFIER), hostname).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["code"].as_str().expect("a code").to_string()
}

/// The exchange carries no credential at all: no cookie, no header.
async fn exchange(code: &str, verifier: &str) -> (StatusCode, Value) {
    call(
        flat_api(),
        "POST",
        "/auth/cli/exchange",
        &[],
        Some(json!({ "code": code, "code_verifier": verifier })),
    )
    .await
}

fn assert_invalid_code((status, body): (StatusCode, Value), why: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
    assert_eq!(body["code"], "invalid_code", "{why}: {body}");
    assert!(body.get("secret").is_none(), "{why}");
}

/// A login is all-access, and an all-access token that outlives an org's
/// lifetime cap is not refused: it goes inert in that org. So a login is
/// minted no longer than the tightest cap among its owner's orgs, and works
/// there.
#[tokio::test]
async fn a_login_lasts_no_longer_than_the_tightest_cap_of_its_owners_orgs() {
    let fx = fixture().await;
    join_org(&fx.db, fx.org_id, fx.user.id, OrgRole::Member).await;
    org_token_policies::ActiveModel {
        org_id: Set(fx.org_id),
        max_lifetime_days: Set(Some(180)),
        allow_all_access_tokens: Set(true),
        require_environment_on_trust_policies: Set(false),
        updated_by: Set(None),
        updated_at: Set(Utc::now().into()),
    }
    .insert(&fx.db)
    .await
    .expect("cap the org's tokens at 180 days");

    let (status, minted) = exchange(&code_for(&fx, "build-box").await, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let token = &minted["token"];
    let expires = DateTime::parse_from_rfc3339(token["expires_at"].as_str().expect("an expiry"))
        .unwrap()
        .with_timezone(&Utc);
    let lifetime = expires - Utc::now();
    assert!(
        lifetime > Duration::days(179) && lifetime <= Duration::days(180),
        "the org's 180 days, not a year: got {lifetime}"
    );
    // And so no org's policy makes it inert: the read the request path uses.
    let id = token["id"]
        .as_str()
        .expect("an id")
        .parse()
        .expect("a uuid");
    let row = super::pat_row(&fx.db, id).await;
    let inert_in = oxy_auth::token::policy_store::blocks(&fx.db, &row, &[])
        .await
        .expect("the policy read");
    assert!(
        inert_in.is_empty(),
        "the login is inert in {} org(s) it was minted to reach",
        inert_in.len()
    );
}

#[tokio::test]
async fn a_login_mints_an_all_access_token_named_for_the_host() {
    let fx = fixture().await;
    let code = code_for(&fx, "build-box").await;

    let (status, minted) = exchange(&code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let token = &minted["token"];
    assert_eq!(token["name"], "oxyc on build-box");
    assert_eq!(token["kind"], "personal");
    assert_eq!(token["source"], "oxyc_login");
    assert_eq!(token["all_access"], true);
    assert_eq!(token["platform"], false, "the user holds no staff standing");
    assert_eq!(token["partner"], false, "nor a partner one");
    assert_eq!(token["owner"]["id"], json!(fx.user.id));
    let expires = DateTime::parse_from_rfc3339(token["expires_at"].as_str().expect("an expiry"))
        .unwrap()
        .with_timezone(&Utc);
    let lifetime = expires - Utc::now();
    assert!(
        lifetime > Duration::days(364) && lifetime <= Duration::days(365),
        "a year, got {lifetime}"
    );

    let secret = minted["secret"].as_str().expect("the secret");
    assert!(secret.starts_with("oxy_pat_"));
    let (status, me) = get_as(secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(me["id"], token["id"]);
}

#[tokio::test]
async fn a_padded_challenge_is_the_same_challenge() {
    let fx = fixture().await;
    let padded = format!("{}=", challenge_of(VERIFIER));
    let (status, body) = authorize_with(&fx, &padded, "padded-host").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, minted) = exchange(body["code"].as_str().unwrap(), VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
}

#[tokio::test]
async fn a_staff_login_carries_the_standing_its_owner_holds() {
    let fx = fixture().await;
    make_staff(&fx.db, fx.user.email.as_deref().unwrap()).await;
    let code = code_for(&fx, "ops-laptop").await;
    let (status, minted) = exchange(&code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    assert_eq!(minted["token"]["platform"], true);
    assert_eq!(minted["token"]["partner"], false);
}

#[tokio::test]
async fn a_wrong_verifier_is_refused_and_spends_the_code() {
    let fx = fixture().await;
    let code = code_for(&fx, "host").await;
    let wrong = "a".repeat(43);
    assert_invalid_code(exchange(&code, &wrong).await, "a wrong verifier");
    assert_invalid_code(
        exchange(&code, VERIFIER).await,
        "the right verifier, after the code was spent by a wrong one",
    );
}

#[tokio::test]
async fn a_code_redeems_once() {
    let fx = fixture().await;
    let code = code_for(&fx, "host").await;
    assert_eq!(exchange(&code, VERIFIER).await.0, StatusCode::OK);
    assert_invalid_code(exchange(&code, VERIFIER).await, "a reused code");
}

#[tokio::test]
async fn an_expired_or_unknown_code_is_refused() {
    let fx = fixture().await;
    assert_invalid_code(
        exchange("never-issued", VERIFIER).await,
        "a code nobody issued",
    );

    let code = code_for(&fx, "host").await;
    // Six minutes on: the five-minute window has closed.
    cli_auth_codes::Entity::update_many()
        .col_expr(
            cli_auth_codes::Column::ExpiresAt,
            Expr::value((Utc::now() - Duration::minutes(1)).fixed_offset()),
        )
        .exec(&fx.db)
        .await
        .expect("age the code");
    assert_invalid_code(exchange(&code, VERIFIER).await, "an expired code");
}

#[tokio::test]
async fn logging_in_again_from_a_host_retires_its_earlier_token() {
    let fx = fixture().await;
    let login = |host: &'static str| {
        let fx = &fx;
        async move {
            let code = code_for(fx, host).await;
            let (status, minted) = exchange(&code, VERIFIER).await;
            assert_eq!(status, StatusCode::OK, "{minted}");
            minted["secret"].as_str().unwrap().to_string()
        }
    };
    let first = login("laptop").await;
    let elsewhere = login("desktop").await;
    assert_eq!(get_as(&first, "/auth/token").await.0, StatusCode::OK);

    let second = login("laptop").await;
    assert_eq!(
        get_as(&first, "/auth/token").await.0,
        StatusCode::UNAUTHORIZED,
        "the earlier `oxyc on laptop` is revoked"
    );
    assert_eq!(get_as(&second, "/auth/token").await.0, StatusCode::OK);
    assert_eq!(
        get_as(&elsewhere, "/auth/token").await.0,
        StatusCode::OK,
        "another host's login is untouched"
    );

    // Revoked, not deleted: the list still shows both laptops.
    let (_, list) = call(
        flat_api(),
        "GET",
        "/user/tokens",
        &[("cookie", &fx.cookie)],
        None,
    )
    .await;
    let laptops: Vec<&str> = list["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|t| t["name"] == "oxyc on laptop")
        .map(|t| t["status"].as_str().unwrap())
        .collect();
    assert_eq!(laptops.len(), 2, "{list}");
    assert!(laptops.contains(&"active") && laptops.contains(&"revoked"));
}

#[tokio::test]
async fn authorize_validates_the_challenge_and_the_hostname() {
    let fx = fixture().await;
    let challenge = challenge_of(VERIFIER);
    for (bad_challenge, bad_host, why) in [
        (
            "not-a-challenge",
            "host".to_string(),
            "a malformed challenge",
        ),
        (challenge.as_str(), String::new(), "an empty hostname"),
        (
            challenge.as_str(),
            "h".repeat(256),
            "a 256-character hostname",
        ),
    ] {
        let (status, body) = authorize_with(&fx, bad_challenge, &bad_host).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
    }
    let (status, _) = authorize_with(&fx, &challenge, &"h".repeat(255)).await;
    assert_eq!(status, StatusCode::OK, "255 characters is the limit");

    // No session, no code.
    let (status, _) = call(
        flat_api(),
        "POST",
        "/auth/cli/authorize",
        &[],
        Some(json!({ "code_challenge": challenge, "hostname": "host" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
