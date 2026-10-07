//! The session-signing key, on the served routes: a session signed with the
//! string every session used to be signed with is nobody's, the deployment's
//! own key lives in one database row, and a key for one purpose signs nothing
//! for another.

use axum::http::StatusCode;
use chrono::Utc;
use entity::prelude::ServerKeys;
use jsonwebtoken::{EncodingKey, Header, encode};
use sea_orm::EntityTrait;
use serde_json::json;

use super::stack::flat_api;
use super::{Fixture, call, fixture};

/// A session JWT for the fixture's user, signed with `key`.
fn session_signed_with(fx: &Fixture, key: &[u8]) -> String {
    let now = Utc::now().timestamp();
    let claims = json!({
        "sub": fx.user.id.to_string(),
        "email": fx.user.email,
        "exp": now + 3600,
        "iat": now,
    });
    encode(&Header::default(), &claims, &EncodingKey::from_secret(key)).expect("sign")
}

async fn orgs_as(cookie: &str) -> StatusCode {
    call(flat_api(), "GET", "/orgs", &[("cookie", cookie)], None)
        .await
        .0
}

#[tokio::test]
async fn a_session_signed_with_the_old_constant_is_nobodys() {
    let fx = fixture().await;
    // Everything a forger needs was public: the user's id and this string.
    let forged = session_signed_with(&fx, b"authentication_secret");

    for carried in [
        vec![("cookie", format!("oxy_session={forged}"))],
        vec![("authorization", forged.clone())],
        vec![("authorization", format!("Bearer {forged}"))],
    ] {
        let headers: Vec<(&str, &str)> = carried.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let (status, _) = call(flat_api(), "GET", "/orgs", &headers, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{:?}", carried[0].0);
    }
    // Nor does its cookie hydrate into a real session.
    let cookie = format!("oxy_session={forged}");
    let (status, _) = call(
        flat_api(),
        "GET",
        "/auth/session",
        &[("cookie", &cookie)],
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    // The control: the same user's real session, minted by this deployment.
    assert_eq!(orgs_as(&fx.cookie).await, StatusCode::OK);
}

#[tokio::test]
async fn the_deployment_keeps_one_secret_in_one_row() {
    let fx = fixture().await;
    // Minting the fixture's session was the first use: the row exists.
    assert_eq!(orgs_as(&fx.cookie).await, StatusCode::OK);

    let rows = ServerKeys::find().all(&fx.db).await.unwrap();
    assert_eq!(rows.len(), 1, "one key, however many requests asked for it");
    assert_eq!(rows[0].name, "session");
    assert_eq!(rows[0].secret.len(), 32);
    assert_ne!(rows[0].secret, vec![0u8; 32]);
    // The row is the root, not the key: signing with it directly signs nothing.
    let with_raw_row = session_signed_with(&fx, &rows[0].secret);
    assert_eq!(
        orgs_as(&format!("oxy_session={with_raw_row}")).await,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn an_oauth_state_is_not_a_session() {
    let fx = fixture().await;
    let (status, issued) = call(flat_api(), "POST", "/auth/oauth/state", &[], None).await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let state = issued["state"].as_str().expect("a state");

    // Signed by this deployment, for another purpose: not a way in.
    assert_eq!(
        orgs_as(&format!("oxy_session={state}")).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(orgs_as(&fx.cookie).await, StatusCode::OK);
}
