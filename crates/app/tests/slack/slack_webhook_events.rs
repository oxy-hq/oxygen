//! Smoke tests for the Slack events webhook handler.
//!
//! These tests call the handler function directly (no HTTP server needed)
//! using axum's `HeaderMap` + `Bytes` types. A Mutex serialises the env-var
//! manipulation required for `SlackConfig::from_env()` — tokio's, because every
//! test holds it across the handler's `.await`.

use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use hmac::{Hmac, KeyInit, Mac};
use oxy_app::api::middlewares::workspace_context::PreaggCacheCtx;
use oxy_app::integrations::slack::webhooks::events::handle_events;
use sha2::Sha256;
use tokio::sync::Mutex;

// ── env-var plumbing ─────────────────────────────────────────────────────────

const TEST_SIGNING_SECRET: &str = "test_signing_secret_e2e";

static ENV_MUTEX: Mutex<()> = Mutex::const_new(());

fn set_slack_env(signing_secret: &str) {
    unsafe {
        std::env::set_var("OXY_SLACK_ENABLED", "true");
        std::env::set_var("OXY_SLACK_CLIENT_ID", "test_client_id");
        std::env::set_var("OXY_SLACK_CLIENT_SECRET", "test_client_secret");
        std::env::set_var("OXY_SLACK_SIGNING_SECRET", signing_secret);
        std::env::set_var("OXY_SLACK_APP_BASE_URL", "https://app.example.com");
    }
}

fn unset_slack_env() {
    unsafe {
        std::env::remove_var("OXY_SLACK_ENABLED");
        std::env::remove_var("OXY_SLACK_CLIENT_ID");
        std::env::remove_var("OXY_SLACK_CLIENT_SECRET");
        std::env::remove_var("OXY_SLACK_SIGNING_SECRET");
        std::env::remove_var("OXY_SLACK_APP_BASE_URL");
    }
}

// ── HMAC helper ──────────────────────────────────────────────────────────────

fn sign_body(secret: &str, ts: i64, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(format!("v0:{ts}:").as_bytes());
    mac.update(body);
    format!("v0={}", hex::encode(mac.finalize().into_bytes()))
}

fn build_headers(ts: i64, sig: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-slack-request-timestamp",
        HeaderValue::from_str(&ts.to_string()).unwrap(),
    );
    headers.insert("x-slack-signature", HeaderValue::from_str(sig).unwrap());
    headers
}

// ── tests ────────────────────────────────────────────────────────────────────

/// Slack sends a url_verification challenge; handler must echo it back.
#[tokio::test]
async fn url_verification_echoes_challenge() {
    let _g = ENV_MUTEX.lock().await;
    set_slack_env(TEST_SIGNING_SECRET);

    let body = br#"{"type":"url_verification","challenge":"abc123"}"#;
    let ts = chrono::Utc::now().timestamp();
    let sig = sign_body(TEST_SIGNING_SECRET, ts, body);
    let headers = build_headers(ts, &sig);

    let response = handle_events(headers, PreaggCacheCtx::default(), Bytes::from_static(body))
        .await
        .into_response();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8_lossy(&body_bytes);
    assert!(
        body_str.contains("abc123"),
        "expected challenge in response, got: {body_str}"
    );

    unset_slack_env();
}

/// An event_callback for an unknown team_id should be silently dropped (200).
#[tokio::test]
async fn unknown_team_id_drops_silently() {
    let _g = ENV_MUTEX.lock().await;
    set_slack_env(TEST_SIGNING_SECRET);

    let body = br#"{
        "type": "event_callback",
        "team_id": "T_NOTINSTALLED",
        "event": {
            "type": "app_mention",
            "user": "U123",
            "text": "hello",
            "ts": "1700000000.000000",
            "channel": "C123"
        }
    }"#;
    let ts = chrono::Utc::now().timestamp();
    let sig = sign_body(TEST_SIGNING_SECRET, ts, body);
    let headers = build_headers(ts, &sig);

    let response = handle_events(
        headers,
        PreaggCacheCtx::default(),
        Bytes::copy_from_slice(body),
    )
    .await
    .into_response();

    // Must be 200 — we don't 4xx unknown teams (Slack would retry forever).
    assert_eq!(response.status(), StatusCode::OK);

    unset_slack_env();
}

/// A request with a bad signature must be rejected with 401.
#[tokio::test]
async fn bad_signature_rejected() {
    let _g = ENV_MUTEX.lock().await;
    set_slack_env(TEST_SIGNING_SECRET);

    let body = br#"{"type":"url_verification","challenge":"xyz"}"#;
    let ts = chrono::Utc::now().timestamp();
    // Deliberately wrong signature.
    let headers = build_headers(
        ts,
        "v0=deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
    );

    let response = handle_events(headers, PreaggCacheCtx::default(), Bytes::from_static(body))
        .await
        .into_response();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    unset_slack_env();
}
