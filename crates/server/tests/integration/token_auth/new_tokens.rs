//! `oxy_pat_` tokens: either header authenticates, and a failing one is 401
//! with no fallthrough to the cookie beside it.

use super::*;
use axum::http::StatusCode;
use chrono::Duration;
use oxy_auth::authenticator::Authenticator;
use oxy_auth::built_in::BuiltInAuthenticator;

#[tokio::test]
async fn a_pat_authenticates_as_bearer_and_as_x_api_key_on_every_surface() {
    let fx = fixture().await;
    let (id, pat) = minted_pat(&fx, None).await;
    let bearer = format!("Bearer {pat}");

    for (name, value) in [
        ("authorization", bearer.as_str()),
        ("x-api-key", pat.as_str()),
    ] {
        for router in [api_surface(), external_surface()] {
            let (status, body) = probe_as(&fx, router, "", &[(name, value)]).await;
            assert_eq!(status, StatusCode::OK, "{name}: {body}");
            assert_eq!(body["user_id"], json!(fx.user.id));
            assert_eq!(
                body["token_id"],
                json!(id),
                "the credential marker names the token"
            );
            assert_eq!(body["kind"], "personal");
        }
    }

    // The custom-app path (subdomain serving, gates, data plane) goes through
    // BuiltInAuthenticator, which goes through the same dispatch.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", bearer.parse().unwrap());
    let identity = BuiltInAuthenticator::new(
        oxy_auth::token::SandboxAgent::Refuse,
        oxy_auth::ports::AuthPlatform::new(oxy_platform::ProcessPlatformDb),
    )
    .authenticate(&headers)
    .await
    .expect("custom-app path");
    assert_eq!(identity.user_id, Some(fx.user.id));
}

#[tokio::test]
async fn a_pat_is_also_accepted_through_api_key_query() {
    let fx = fixture().await;
    let (_, pat) = minted_pat(&fx, None).await;
    let query = format!("?api_key={pat}");
    for router in [api_surface(), external_surface()] {
        let (status, _) = probe_as(&fx, router, &query, &[]).await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn the_cookie_alone_is_a_session_not_a_credential() {
    let fx = fixture().await;
    let (status, body) = probe_as(&fx, api_surface(), "", &[("cookie", &fx.cookie)]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["token_id"], Value::Null);
}

/// Every way a pat can be bad, each sent beside a valid cookie in both
/// headers. None may fall through to the cookie.
async fn assert_refused_despite_cookie(fx: &Fixture, pat: &str, why: &str) {
    let bearer = format!("Bearer {pat}");
    for (name, value) in [("authorization", bearer.as_str()), ("x-api-key", pat)] {
        let headers = [("cookie", fx.cookie.as_str()), (name, value)];
        let (status, _) = probe_as(fx, api_surface(), "", &headers).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{why} via {name}");
    }
}

#[tokio::test]
async fn a_bad_pat_is_401_even_with_a_valid_cookie() {
    let fx = fixture().await;
    // Well-formed and checksummed, but never stored.
    let unknown = oxy_auth::token::generate_personal().plaintext;
    assert_refused_despite_cookie(&fx, &unknown, "unknown").await;
    // Fails its checksum.
    let mut broken = minted_pat(&fx, None).await.1;
    let last = broken.pop().unwrap();
    broken.push(if last == 'A' { 'B' } else { 'A' });
    assert_refused_despite_cookie(&fx, &broken, "bad checksum").await;
}

#[tokio::test]
async fn a_revoked_pat_is_401_even_with_a_valid_cookie() {
    let fx = fixture().await;
    let (id, pat) = minted_pat(&fx, None).await;
    let row = pat_row(&fx.db, id).await;
    oxy_auth::token::personal::revoke(&fx.db, row, fx.user.id, "owner")
        .await
        .expect("revoke");
    assert_refused_despite_cookie(&fx, &pat, "revoked").await;
}

#[tokio::test]
async fn an_expired_pat_is_401_even_with_a_valid_cookie() {
    let fx = fixture().await;
    let (_, pat) = minted_pat(&fx, Some(Utc::now() - Duration::minutes(1))).await;
    assert_refused_despite_cookie(&fx, &pat, "expired").await;
}

/// §4.7, refuse what you cannot enforce: a row edited into a shape this
/// release does not implement fails closed rather than being guessed at. The
/// two that matter are a personal row claiming to be a service account — whose
/// standing comes from an account row that does not exist — and one stamped as
/// the `api_keys` mirror of a removed legacy key.
#[tokio::test]
async fn a_row_edited_into_a_shape_this_release_cannot_enforce_is_refused() {
    let fx = fixture().await;
    for (column, value) in [("kind", "'service_account'"), ("kind", "'legacy_key'")] {
        let (id, pat) = minted_pat(&fx, None).await;
        let (ok, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &pat)]).await;
        assert_eq!(ok, StatusCode::OK);

        use sea_orm::ConnectionTrait;
        fx.db
            .execute_unprepared(&format!(
                "UPDATE api_tokens SET {column} = {value} WHERE id = '{id}'"
            ))
            .await
            .unwrap();
        oxy_auth::token::cache::clear(); // as another pod, or 30 s later
        let (status, _) = probe_as(&fx, api_surface(), "", &[("x-api-key", &pat)]).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{column} = {value}");
    }
}
