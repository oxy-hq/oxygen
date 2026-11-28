//! The Oxygen mobile app's sign-in end to end: the browser trades the app's
//! S256 challenge and `oxygen://` redirect URI for a one-time code under its
//! session, and the app — holding no credential yet — redeems `code +
//! verifier + redirect_uri` for a login session JWT, which it later renews.
//!
//! Every way of failing an exchange answers the same 400 `invalid_code`, any
//! attempt spends the code, and the code never crosses over to `oxyc login`.
//! Frontline crew are refused: their sign-in stays on the enrolled kiosk.

use axum::http::StatusCode;
use chrono::Duration;
use entity::{org_frontline_members, users};
use oxy_auth::token::cli_login::challenge_of;
use sea_orm::{ActiveModelTrait, ActiveValue};
use serde_json::{Value, json};
use uuid::Uuid;

use super::stack::flat_api;
use super::{Fixture, call, fixture, minted_pat};

/// RFC 7636 appendix B's verifier.
const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
const REDIRECT: &str = "oxygen://auth";

async fn authorize_with(fx: &Fixture, body: Value) -> (StatusCode, Value) {
    call(
        flat_api(),
        "POST",
        "/auth/mobile/authorize",
        &[("cookie", &fx.cookie)],
        Some(body),
    )
    .await
}

/// A code for [`VERIFIER`] and [`REDIRECT`], issued to the fixture's session.
async fn code_for(fx: &Fixture) -> String {
    let body = json!({
        "code_challenge": challenge_of(VERIFIER),
        "code_challenge_method": "S256",
        "redirect_uri": REDIRECT,
    });
    let (status, body) = authorize_with(fx, body).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["code"].as_str().expect("a code").to_string()
}

/// The exchange carries no credential at all: no cookie, no header.
async fn exchange(code: &str, verifier: &str, redirect_uri: &str) -> (StatusCode, Value) {
    let body = json!({ "code": code, "code_verifier": verifier, "redirect_uri": redirect_uri });
    call(flat_api(), "POST", "/auth/mobile/exchange", &[], Some(body)).await
}

async fn refresh(authorization: Option<&str>) -> (StatusCode, Value) {
    let headers: Vec<(&str, &str)> = authorization
        .map(|value| vec![("authorization", value)])
        .unwrap_or_default();
    call(flat_api(), "POST", "/auth/mobile/refresh", &headers, None).await
}

fn assert_invalid_code((status, body): (StatusCode, Value), why: &str) {
    assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {body}");
    assert_eq!(body["code"], "invalid_code", "{why}: {body}");
    assert!(body.get("token").is_none(), "{why}");
}

/// What only a login session may call answers this bearer.
async fn session_only_status(jwt: &str) -> StatusCode {
    let bearer = format!("Bearer {jwt}");
    let headers = [("authorization", bearer.as_str())];
    call(flat_api(), "GET", "/user/tokens", &headers, None)
        .await
        .0
}

#[tokio::test]
async fn the_app_gets_a_login_session_and_its_cookie() {
    let fx = fixture().await;
    let (status, signed_in) = exchange(&code_for(&fx).await, VERIFIER, REDIRECT).await;
    assert_eq!(status, StatusCode::OK, "{signed_in}");

    assert_eq!(signed_in["user"]["id"], json!(fx.user.id));
    assert_eq!(signed_in["user"]["email"], json!(fx.user.email));
    assert_eq!(signed_in["user"]["name"], json!(fx.user.name));
    let thirty_days = 30 * 24 * 60 * 60;
    let expires_in = signed_in["expires_in"].as_i64().expect("expires_in");
    assert!(
        (thirty_days - 5..=thirty_days).contains(&expires_in),
        "{expires_in}"
    );

    let cookie = &signed_in["session_cookie"];
    assert_eq!(cookie["name"], "oxy_session");
    assert_eq!(cookie["path"], "/");
    assert_eq!(cookie["http_only"], true);
    assert_eq!(cookie["same_site"], "Lax");
    assert!(cookie["expires_at"].is_string(), "{cookie}");

    // A login session, not a token: a session-only route answers it.
    let token = signed_in["token"].as_str().expect("a token");
    assert_eq!(session_only_status(token).await, StatusCode::OK);
}

#[tokio::test]
async fn a_code_is_spent_by_any_attempt() {
    let fx = fixture().await;
    let code = code_for(&fx).await;
    assert_eq!(exchange(&code, VERIFIER, REDIRECT).await.0, StatusCode::OK);
    assert_invalid_code(exchange(&code, VERIFIER, REDIRECT).await, "a replay");

    // A wrong verifier burns the code, so the right one finds nothing.
    let code = code_for(&fx).await;
    assert_invalid_code(
        exchange(&code, &"a".repeat(43), REDIRECT).await,
        "wrong verifier",
    );
    assert_invalid_code(
        exchange(&code, VERIFIER, REDIRECT).await,
        "after a wrong verifier",
    );
}

#[tokio::test]
async fn a_code_is_bound_to_its_redirect_uri() {
    let fx = fixture().await;
    let code = code_for(&fx).await;
    assert_invalid_code(
        exchange(&code, VERIFIER, "oxygen://elsewhere").await,
        "another redirect URI",
    );
    assert_invalid_code(
        exchange(&code, VERIFIER, REDIRECT).await,
        "spent by the attempt",
    );
    assert_invalid_code(
        exchange("no-such-code", VERIFIER, REDIRECT).await,
        "unknown",
    );
}

#[tokio::test]
async fn a_mobile_code_and_an_oxyc_code_never_cross() {
    let fx = fixture().await;
    let mobile = code_for(&fx).await;
    let body = json!({ "code": mobile, "code_verifier": VERIFIER });
    let (status, minted) = call(flat_api(), "POST", "/auth/cli/exchange", &[], Some(body)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{minted}");
    assert!(minted.get("secret").is_none(), "no token for a mobile code");

    let body = json!({ "code_challenge": challenge_of(VERIFIER), "hostname": "laptop" });
    let (status, issued) = call(
        flat_api(),
        "POST",
        "/auth/cli/authorize",
        &[("cookie", &fx.cookie)],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let cli = issued["code"].as_str().expect("a code");
    assert_invalid_code(exchange(cli, VERIFIER, REDIRECT).await, "an oxyc code");
}

#[tokio::test]
async fn only_a_browser_session_approves() {
    let fx = fixture().await;
    let (_, secret) = minted_pat(&fx, None).await;
    let bearer = format!("Bearer {secret}");
    let body = json!({
        "code_challenge": challenge_of(VERIFIER),
        "code_challenge_method": "S256",
        "redirect_uri": REDIRECT,
    });
    let (status, refused) = call(
        flat_api(),
        "POST",
        "/auth/mobile/authorize",
        &[("authorization", &bearer)],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "session_required");
}

#[tokio::test]
async fn an_approval_names_what_it_refuses() {
    let fx = fixture().await;
    let ask = |method: &str, redirect: &str| {
        json!({
            "code_challenge": challenge_of(VERIFIER),
            "code_challenge_method": method,
            "redirect_uri": redirect,
        })
    };
    for redirect in ["exp://192.168.1.4:8081/--/auth", "https://evil.example.com"] {
        let (status, body) = authorize_with(&fx, ask("S256", redirect)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{redirect}: {body}");
        assert_eq!(body["code"], "invalid_redirect_uri", "{redirect}");
    }
    let (status, body) = authorize_with(&fx, ask("plain", REDIRECT)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(body["code"], "invalid_request");
}

#[tokio::test]
async fn a_live_session_is_renewed_and_nothing_else_is() {
    let fx = fixture().await;
    let (_, signed_in) = exchange(&code_for(&fx).await, VERIFIER, REDIRECT).await;
    let token = signed_in["token"].as_str().expect("a token");

    for form in [format!("Bearer {token}"), token.to_string()] {
        let (status, renewed) = refresh(Some(&form)).await;
        assert_eq!(status, StatusCode::OK, "{renewed}");
        let fresh = renewed["token"].as_str().expect("a fresh token");
        assert_eq!(session_only_status(fresh).await, StatusCode::OK);
        assert_eq!(renewed["session_cookie"]["name"], "oxy_session");
    }

    // A personal access token is no session to renew, and nor is nothing.
    let (_, secret) = minted_pat(&fx, None).await;
    for refused in [
        refresh(Some(&format!("Bearer {secret}"))).await,
        refresh(None).await,
    ] {
        assert_eq!(refused.0, StatusCode::UNAUTHORIZED, "{}", refused.1);
        assert_eq!(refused.1["code"], "invalid_session");
    }
}

/// A frontline worker enrolled in the fixture's org — a `users` row with no
/// mailbox and no membership — and the twelve-hour session a kiosk mints them.
async fn crew(fx: &Fixture) -> (Uuid, String) {
    let id = Uuid::new_v4();
    let user = users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(None),
        name: ActiveValue::Set("Shift Lead".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("seed a worker");
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(fx.org_id),
        user_id: ActiveValue::Set(id),
        status: ActiveValue::Set(org_frontline_members::STATUS_ACTIVE.into()),
        ..Default::default()
    }
    .insert(&fx.db)
    .await
    .expect("enrol the worker");
    let jwt = oxy_app_core::session::create_auth_token_with_ttl(
        &oxy_auth::ports::AuthPlatform::new(oxy_platform::ProcessPlatformDb),
        user,
        Duration::hours(12),
    )
    .await
    .expect("mint a shift session");
    (id, format!("oxy_session={jwt}"))
}

#[tokio::test]
async fn a_crew_session_cannot_approve_a_mobile_sign_in() {
    let fx = fixture().await;
    let (_, shift) = crew(&fx).await;
    let body = json!({
        "code_challenge": challenge_of(VERIFIER),
        "code_challenge_method": "S256",
        "redirect_uri": REDIRECT,
    });
    let (status, refused) = call(
        flat_api(),
        "POST",
        "/auth/mobile/authorize",
        &[("cookie", &shift)],
        Some(body),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{refused}");
    assert_eq!(refused["code"], "frontline_not_supported");
    assert_eq!(
        refused["error"],
        "Crew sign-in works only on a store's enrolled device."
    );
}

#[tokio::test]
async fn a_crew_members_code_from_before_the_refusal_is_not_honoured() {
    let fx = fixture().await;
    let (worker, _) = crew(&fx).await;
    // Minted straight into the store, as `authorize` did before it refused crew.
    let code =
        oxy_auth::token::mobile_login::authorize(&fx.db, worker, &challenge_of(VERIFIER), REDIRECT)
            .await
            .expect("store a code");
    assert_invalid_code(
        exchange(&code, VERIFIER, REDIRECT).await,
        "a crew member's code",
    );
}

#[tokio::test]
async fn a_crew_shift_session_cannot_be_renewed_off_the_kiosk() {
    let fx = fixture().await;
    let (_, shift) = crew(&fx).await;
    // The kiosk's PIN login hands the shift JWT back in its body; refresh must
    // not turn it into a session that outlives the shift.
    let jwt = shift.trim_start_matches("oxy_session=");
    let (status, refused) = refresh(Some(&format!("Bearer {jwt}"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{refused}");
    assert_eq!(refused["code"], "invalid_session");
}
