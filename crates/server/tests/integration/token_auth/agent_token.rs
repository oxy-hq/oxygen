//! Minting an **agent token** (API-tokens design, "The agent token
//! (2026-10-07)"): the PKCE exchange of `oxyc login`, carrying a `mint` of
//! `kind: "agent"`.
//!
//! The browser approves under its session; the CLI, holding no credential,
//! redeems the code for an ordinary all-access `oxy_pat_` that is the agent's
//! own — named for it, alive for hours, of source `oxyc_agent`. The standing
//! it carries is in `agent_token_standing`, and what holds once it exists in
//! `agent_token_rules`. An `authorize` with no `mint` is `oxyc login`
//! (`cli_login`), and any other `mint` a sandbox agent token's
//! (`sandbox_agent_cli`).

use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use entity::{api_tokens, cli_auth_codes};
use oxy_auth::token::cli_login::challenge_of;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde_json::{Value, json};
use uuid::Uuid;

use super::sandbox_agent::{assert_refused, grants_of, post_as};
use super::service_accounts::{admin_fixture, in_session, workspace_status};
use super::stack::{flat_api, get_as, get_in_session};
use super::{audit_rows, call, fixture};

/// RFC 7636 appendix B's verifier.
pub(super) const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";

/// A `mint` that asks for an agent token, with `extra` over the bare kind.
pub(super) fn agent_mint(extra: Value) -> Value {
    let mut mint = json!({ "kind": "agent" });
    if let (Value::Object(mint), Value::Object(extra)) = (&mut mint, extra) {
        mint.extend(extra);
    }
    mint
}

/// `POST /auth/cli/authorize` under a browser session.
pub(super) async fn authorize(
    cookie: &str,
    hostname: &str,
    mint: Option<Value>,
) -> (StatusCode, Value) {
    let mut body = json!({ "code_challenge": challenge_of(VERIFIER), "hostname": hostname });
    if let Some(mint) = mint {
        body["mint"] = mint;
    }
    post_as(cookie, "/auth/cli/authorize", body).await
}

/// The exchange carries no credential at all.
pub(super) async fn exchange(code: &str, verifier: &str) -> (StatusCode, Value) {
    call(
        flat_api(),
        "POST",
        "/auth/cli/exchange",
        &[],
        Some(json!({ "code": code, "code_verifier": verifier })),
    )
    .await
}

/// Approve `mint` under `cookie` and redeem the code: the exchange's body.
/// `None` is `oxyc login`.
pub(super) async fn redeemed(cookie: &str, hostname: &str, mint: Option<Value>) -> Value {
    let (status, body) = authorize(cookie, hostname, mint).await;
    assert_eq!(status, StatusCode::OK, "authorize: {body}");
    let (status, minted) = exchange(body["code"].as_str().expect("a code"), VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "exchange: {minted}");
    minted
}

/// An agent token minted for `extra` under `cookie`.
pub(super) async fn minted(cookie: &str, hostname: &str, extra: Value) -> Value {
    redeemed(cookie, hostname, Some(agent_mint(extra))).await
}

pub(super) fn secret_of(minted: &Value) -> String {
    minted["secret"].as_str().expect("the secret").to_string()
}

pub(super) fn id_of(minted: &Value) -> Uuid {
    Uuid::parse_str(minted["token"]["id"].as_str().expect("an id")).unwrap()
}

pub(super) fn expiry_of(token: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(token["expires_at"].as_str().expect("an expiry"))
        .unwrap()
        .with_timezone(&Utc)
}

/// The codes this test's database holds that mint something.
pub(super) async fn mint_codes(db: &DatabaseConnection) -> Vec<cli_auth_codes::Model> {
    let minting = |row: &cli_auth_codes::Model| {
        let kind = row.mint.as_ref().and_then(|mint| mint["kind"].as_str());
        matches!(kind, Some("agent" | "sandbox_agent"))
    };
    let rows = cli_auth_codes::Entity::find().all(db).await.unwrap();
    rows.into_iter().filter(minting).collect()
}

fn assert_lifetime(token: &Value, hours: i64) {
    let lifetime = expiry_of(token) - Utc::now();
    let asked = Duration::hours(hours);
    assert!(
        lifetime > asked - Duration::minutes(1) && lifetime <= asked,
        "{hours} hours, got {lifetime}"
    );
}

#[tokio::test]
async fn an_approved_mint_is_an_all_access_personal_token_of_the_agents_own() {
    let fx = admin_fixture().await;
    let (status, approved) = authorize(&fx.cookie, "build-box", Some(agent_mint(json!({})))).await;
    assert_eq!(status, StatusCode::OK, "{approved}");
    let code = approved["code"].as_str().expect("a code").to_string();
    assert_eq!(approved, json!({ "code": code }), "the login's own answer");

    let (status, minted) = exchange(&code, VERIFIER).await;
    assert_eq!(status, StatusCode::OK, "{minted}");
    let secret = secret_of(&minted);
    assert!(secret.starts_with("oxy_pat_"), "{secret}");
    let token = &minted["token"];
    assert_eq!(token["kind"], "personal");
    assert_eq!(token["source"], "oxyc_agent");
    assert_eq!(token["name"], "agent on build-box");
    assert_eq!(token["all_access"], true);
    assert_eq!(token["grants"], json!([]));
    assert_eq!(token["platform"], false, "no standing was asked for");
    assert_eq!(token["partner"], false);
    assert_eq!(token["owner"]["id"], json!(fx.user.id));
    assert_eq!(token["status"], "active");
    assert_lifetime(token, 8);
    assert!(grants_of(&fx.db, id_of(&minted)).await.is_empty());

    // It describes itself as what it is, with nothing of a sandbox agent's.
    let (status, me) = get_as(&secret, "/auth/token").await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["id"], token["id"]);
    assert_eq!(me["kind"], "personal");
    assert_eq!(me["source"], "oxyc_agent");
    assert!(me.get("minter").is_none() && me.get("apps").is_none());

    // An ordinary all-access token: it reaches what its owner reaches.
    assert_eq!(
        workspace_status(&secret, fx.workspace_id, "GET", "read").await,
        StatusCode::NO_CONTENT
    );
    // And its owner sees it beside their other tokens, told apart by source.
    let (_, list) = in_session(&fx.cookie, "GET", "/user/tokens", None).await;
    let listed = list["tokens"].as_array().expect("tokens");
    assert!(
        listed
            .iter()
            .any(|t| t["id"] == token["id"] && t["source"] == "oxyc_agent"),
        "{list}"
    );

    // Single use, like any code.
    let (status, again) = exchange(&code, VERIFIER).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(again["code"], "invalid_code");
}

#[tokio::test]
async fn the_lifetime_is_the_hours_asked_for_from_one_to_a_week() {
    let fx = fixture().await;
    for hours in [1, 24, 168] {
        let minted = minted(&fx.cookie, "host", json!({ "expires_in_hours": hours })).await;
        assert_lifetime(&minted["token"], hours);
    }
    // Left out, or null: eight.
    let unset = minted(&fx.cookie, "host", json!({ "expires_in_hours": null })).await;
    assert_lifetime(&unset["token"], 8);
}

#[tokio::test]
async fn the_name_defaults_to_the_host_and_a_sent_name_is_kept() {
    let fx = fixture().await;
    let unnamed = minted(&fx.cookie, "MacBookPro", json!({})).await;
    assert_eq!(unnamed["token"]["name"], "agent on MacBookPro");
    let named = minted(
        &fx.cookie,
        "MacBookPro",
        json!({ "name": "  nightly repro " }),
    )
    .await;
    assert_eq!(named["token"]["name"], "nightly repro");

    // A host too long to name a token after needs a name sent, and says so.
    let long_host = "h".repeat(255);
    let refused = authorize(&fx.cookie, &long_host, Some(agent_mint(json!({})))).await;
    assert_refused(
        refused,
        StatusCode::BAD_REQUEST,
        "invalid_agent_token",
        "a default name past the limit",
    );
    let named = minted(&fx.cookie, &long_host, json!({ "name": "agent" })).await;
    assert_eq!(named["token"]["name"], "agent");
}

#[tokio::test]
async fn a_mint_outside_the_contract_answers_400_and_issues_no_code() {
    let fx = fixture().await;
    for (extra, why) in [
        (json!({ "expires_in_hours": 0 }), "no hours"),
        (json!({ "expires_in_hours": 169 }), "past a week"),
        (json!({ "expires_in_hours": -8 }), "negative hours"),
        (json!({ "expires_in_hours": "8" }), "hours as a string"),
        (json!({ "expires_in_hours": 8.5 }), "half an hour"),
        (
            json!({ "standing": "true" }),
            "a standing that is no boolean",
        ),
        (json!({ "name": "n".repeat(101) }), "a name past 100"),
        (json!({ "name": "  " }), "an empty name"),
        (json!({ "name": 7 }), "a name that is no string"),
        (json!({ "read_only": true }), "an unknown field"),
        (json!({ "orgs": [fx.org_id] }), "narrowing by org"),
        (
            json!({ "apps": [Uuid::new_v4()] }),
            "a sandbox agent's field",
        ),
        (json!({ "all_access": false }), "a personal token's field"),
        (json!({ "platform": true }), "a personal token's standing"),
        (json!({ "grants": [] }), "a personal token's grants"),
        (
            json!({ "expires_in_days": 30 }),
            "a personal token's expiry",
        ),
    ] {
        let refused = authorize(&fx.cookie, "host", Some(agent_mint(extra))).await;
        assert!(refused.1["error"].is_string(), "{why}: {}", refused.1);
        assert_refused(refused, StatusCode::BAD_REQUEST, "invalid_agent_token", why);
    }
    assert!(
        mint_codes(&fx.db).await.is_empty(),
        "a refused mint issues no code"
    );
    let tokens = api_tokens::Entity::find().all(&fx.db).await.unwrap();
    assert!(tokens.is_empty(), "and mints nothing");
}

#[tokio::test]
async fn the_token_routes_do_not_mint_one() {
    // Only an approval through `authorize` does: `POST /user/tokens` knows
    // the personal and the sandbox agent kinds, as before.
    let fx = fixture().await;
    let body = json!({ "name": "agent", "kind": "agent" });
    let (status, refused) = in_session(&fx.cookie, "POST", "/user/tokens", Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    assert!(refused.get("secret").is_none());
    let tokens = api_tokens::Entity::find().all(&fx.db).await.unwrap();
    assert!(tokens.is_empty());
}

#[tokio::test]
async fn a_wrong_verifier_spends_the_code_and_mints_nothing() {
    let fx = fixture().await;
    let (_, approved) = authorize(&fx.cookie, "host", Some(agent_mint(json!({})))).await;
    let code = approved["code"].as_str().expect("a code");
    for (verifier, why) in [
        ("a".repeat(43), "a wrong verifier"),
        (VERIFIER.into(), "spent"),
    ] {
        let (status, refused) = exchange(code, &verifier).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {refused}");
        assert_eq!(refused["code"], "invalid_code", "{why}");
        assert!(refused.get("secret").is_none(), "{why}");
    }
    let tokens = api_tokens::Entity::find().all(&fx.db).await.unwrap();
    assert!(tokens.is_empty(), "nothing was minted");
}

#[tokio::test]
async fn the_code_is_stored_where_a_login_lookup_cannot_find_it() {
    // What a binary one release back depends on: it finds a code by the
    // SHA-256 of the code alone and mints the engineer's own login token, the
    // long-lived one, for whatever it finds. An agent's code must not be
    // findable that way.
    let fx = fixture().await;
    let (_, approved) = authorize(
        &fx.cookie,
        "host",
        Some(agent_mint(json!({ "standing": true }))),
    )
    .await;
    let code = approved["code"].as_str().expect("a code");
    let by_login_hash = cli_auth_codes::Entity::find_by_id(oxy_auth::token::hash_token(code))
        .one(&fx.db)
        .await
        .unwrap();
    assert!(
        by_login_hash.is_none(),
        "an agent's code under the login hash"
    );

    let stored = mint_codes(&fx.db).await;
    assert_eq!(stored.len(), 1);
    // What was approved, the name resolved. That binary reads any mint as a
    // sandbox agent token's, which this is not: it answers `invalid_code`.
    assert_eq!(
        stored[0].mint,
        Some(json!({
            "kind": "agent",
            "name": "agent on host",
            "standing": true,
            "expires_in_hours": 8,
        }))
    );
}

#[tokio::test]
async fn token_options_carry_the_agent_tokens_limits() {
    let fx = fixture().await;
    let (status, options) = get_in_session(&fx, "/user/token-options").await;
    assert_eq!(status, StatusCode::OK, "{options}");
    assert_eq!(
        options["agent"],
        json!({ "default_hours": 8, "max_hours": 168 })
    );
    // Beside the sandbox agent token's, which is as it was.
    assert_eq!(
        options["sandbox_agent"],
        json!({ "default_hours": 8, "max_hours": 168, "max_apps": 5 })
    );
}

#[tokio::test]
async fn the_created_row_tells_an_agent_token_from_a_login() {
    let fx = admin_fixture().await;
    redeemed(&fx.cookie, "build-box", None).await;
    let agent = minted(&fx.cookie, "build-box", json!({ "standing": true })).await;

    let rows = audit_rows(&fx.db, "token.created").await;
    assert_eq!(
        rows.len(),
        2,
        "one row each, in the one org their owner is in"
    );
    let of = |source: &str| {
        rows.iter()
            .find(|row| row.metadata["source"] == source)
            .unwrap_or_else(|| panic!("a row of source {source}"))
    };
    let (login, row) = (of("oxyc_login"), of("oxyc_agent"));
    // The same engineer, the same host: the source is what tells them apart.
    assert_eq!(login.metadata["hostname"], "build-box");
    assert_eq!(row.metadata["hostname"], "build-box");
    assert_eq!(row.actor_user_id, Some(fx.user.id));
    assert_eq!(row.org_id, Some(fx.org_id));
    assert_eq!(row.metadata["token_id"], agent["token"]["id"]);
    assert_eq!(row.metadata["token_kind"], "personal");
    assert_eq!(row.metadata["all_access"], true);
    assert_eq!(row.metadata["grants"], json!([]));
    assert!(row.metadata["expires_at"].is_string(), "{}", row.metadata);
    // A standing was approved and its owner held none: both are on the row.
    assert_eq!(row.metadata["standing_approved"], true);
    assert_eq!(row.metadata["platform"], false);
    assert_eq!(row.metadata["partner"], false);
    assert!(!row.metadata.to_string().contains(&secret_of(&agent)));
    // The login's row is as it was.
    assert!(login.metadata.get("standing_approved").is_none());
}
