//! The login response of a **token session** — the browser session a personal
//! access token opens with a ticket (`oxy_auth::token::browser_session`; the
//! routes are `api::user_tokens::browser_session`).
//!
//! It answers in the shape every login does, so the web app stores it the way
//! it stores any session. Two things differ, and both are the point:
//!
//! - the payload is what the **token** reaches — its orgs, its platform flags
//!   — not what its owner does;
//! - the JWT is never re-minted here. A login's cookie hydrates into a fresh
//!   thirty-day session (`get_session`); a token session's hydrates as itself,
//!   or a narrow, twelve-hour credential would trade up to its owner's login.

use axum::Json;
use axum::http::{HeaderMap, StatusCode, header::SET_COOKIE};
use entity::prelude::Users;
use entity::users::{self, UserStatus};
use oxy::database::client::establish_connection;
use oxy_auth::token::CredentialContext;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{DatabaseConnection, EntityTrait};

use super::dto::AuthResponse;
use super::ops::{build_session_cookie_with_max_age, is_request_secure, session_payload};
use crate::server::authz::Caller;

/// The response that hands `jwt` to a browser as `user`'s session under
/// `credential`. `cookie_max_age_secs` sets the `oxy_session` cookie to lapse
/// with the JWT; `None` leaves the cookie alone, for a browser that already
/// holds it.
pub(crate) async fn token_session_response(
    request_headers: &HeaderMap,
    user: &users::Model,
    credential: &CredentialContext,
    jwt: String,
    cookie_max_age_secs: Option<i64>,
    connection: &DatabaseConnection,
) -> Result<(HeaderMap, Json<AuthResponse>), StatusCode> {
    let caller = Caller::of(&AuthenticatedUser::from(user.clone()), Some(credential));
    let (user_info, orgs) = session_payload(user, &caller, connection).await?;

    let mut response_headers = HeaderMap::new();
    if let Some(max_age_secs) = cookie_max_age_secs {
        let cookie = build_session_cookie_with_max_age(
            &jwt,
            is_request_secure(request_headers),
            max_age_secs,
        );
        match cookie.parse() {
            Ok(value) => {
                response_headers.insert(SET_COOKIE, value);
            }
            Err(_) => tracing::error!("Failed to build session cookie header value"),
        }
    }
    Ok((
        response_headers,
        Json(AuthResponse {
            token: jwt,
            user: user_info,
            orgs,
        }),
    ))
}

/// `GET /auth/session` for a cookie that holds a token session: the same JWT
/// back, with the token's payload, once the session has authenticated as any
/// request of its would. `401` when it no longer does — the token was revoked
/// or expired, or the session lapsed.
pub(super) async fn hydrate(
    request_headers: &HeaderMap,
    jwt: String,
) -> Result<(HeaderMap, Json<AuthResponse>), StatusCode> {
    let (_, credential) = oxy_auth::token::authenticate_browser_session(&jwt)
        .await
        .map_err(|e| {
            tracing::debug!("session hydrate: token session rejected: {e}");
            StatusCode::UNAUTHORIZED
        })?;
    let connection = establish_connection().await.map_err(|e| {
        tracing::error!("session hydrate: db connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let user = Users::find_by_id(credential.principal_user_id)
        .one(&connection)
        .await
        .map_err(|e| {
            tracing::error!("session hydrate: user lookup failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .filter(|u| u.status == UserStatus::Active)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    token_session_response(request_headers, &user, &credential, jwt, None, &connection).await
}
