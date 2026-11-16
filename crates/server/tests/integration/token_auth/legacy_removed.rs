//! Legacy API keys were removed. This is what a key still deployed somewhere
//! now gets.
//!
//! The two files this replaces (`legacy_keys`, `legacy_reach`) asserted the
//! opposite: that an `oxy_<32 hex>` key kept working, reached every org its
//! owner was in, and could be extended and revoked through
//! `/{workspace_id}/api-keys`. None of that is true any more, and the point of
//! keeping a file here is that the *absence* is covered — a change that
//! quietly let one authenticate again would otherwise pass an empty directory.

use super::*;
use axum::http::StatusCode;
use chrono::Duration;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;

/// The body a refused legacy key gets. It names the replacement, so an
/// integration that has been sending the same key for a year learns what to do
/// from the 401 alone.
fn says_it_was_removed(body: &Value) -> bool {
    let text = body.to_string();
    text.contains("Legacy API keys") && text.contains("were removed")
}

/// Every surface a legacy key used to reach, refusing it the same way.
async fn assert_refused_everywhere(fx: &Fixture, key: &str) {
    let bearer = format!("Bearer {key}");
    for (surface, router) in [("api", api_surface()), ("external", external_surface())] {
        for (how, headers) in [
            ("X-API-Key", vec![("x-api-key", key)]),
            ("bearer", vec![("authorization", bearer.as_str())]),
        ] {
            let (status, body) = probe_as(fx, router.clone(), "", &headers).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{surface} {how}: {body}");
            assert!(says_it_was_removed(&body), "{surface} {how}: {body}");
        }
    }
    // `?api_key=` is promoted into the header, so it refuses identically. The
    // query form stays — it is how EventSource carries a *token*.
    let (status, _) = probe_as(fx, api_surface(), &format!("?api_key={key}"), &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The custom-app path authenticates inside its handlers, not through the
    // `/api` layers, so it is checked on its own.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-api-key", key.parse().unwrap());
    let refused = BuiltInAuthenticator::new(oxy_auth::token::SandboxAgent::Refuse)
        .authenticate(&headers)
        .await;
    assert!(refused.is_err(), "the custom-app path admits no legacy key");
}

#[tokio::test]
async fn a_leftover_legacy_key_authenticates_nothing_and_is_told_why() {
    let fx = fixture().await;
    let (_, key) = legacy_key(&fx, None).await;
    assert_refused_everywhere(&fx, &key).await;
}

/// The key the backfill mirrored into `api_tokens` is the common shape in a
/// deployment that has been running a while: live row, years of expiry left.
/// The mirror makes no difference — admission refuses it by kind, before it
/// judges whether the row is revoked or expired.
#[tokio::test]
async fn a_mirrored_legacy_key_is_refused_too() {
    let fx = fixture().await;
    let (_, key) = mirrored_legacy_key(&fx, Some(Utc::now() + Duration::days(365))).await;
    assert_refused_everywhere(&fx, &key).await;
}

/// The surface that minted, listed, extended and revoked a legacy key is gone.
/// A 404 is the honest answer; a 403 would suggest it is still there behind a
/// permission.
#[tokio::test]
async fn the_api_keys_routes_are_gone() {
    let fx = fixture().await;
    let (id, _) = legacy_key(&fx, None).await;
    let cookie = fx.cookie.clone();
    for (method, path) in [
        ("GET", String::new()),
        ("POST", String::new()),
        ("GET", format!("/{id}")),
        ("DELETE", format!("/{id}")),
        ("POST", format!("/{id}/extend")),
        ("GET", format!("/{id}/activity")),
    ] {
        let uri = format!("/{}/api-keys{path}", fx.workspace_id);
        let (status, _) = call(
            api_surface(),
            method,
            &uri,
            &[("cookie", cookie.as_str())],
            Some(json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{method} {uri}");
    }
}

/// `X-API-Key` itself still works — it is how `/external/api` is reached. Only
/// the legacy *format* went, so a personal access token in that header
/// authenticates exactly as it did.
#[tokio::test]
async fn x_api_key_still_carries_a_token() {
    let fx = fixture().await;
    let (id, pat) = minted_pat(&fx, None).await;
    for (surface, router) in [("api", api_surface()), ("external", external_surface())] {
        let (status, body) = probe_as(&fx, router, "", &[("x-api-key", pat.as_str())]).await;
        assert_eq!(status, StatusCode::OK, "{surface}: {body}");
        assert_eq!(body["token_id"], json!(id), "{surface}");
        assert_eq!(body["kind"], "personal", "{surface}");
    }
}
