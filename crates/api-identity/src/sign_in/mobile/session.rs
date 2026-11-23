//! Reading and renewing the login session the app holds.
//!
//! The app's credential is the same JWT a browser login yields, so it is read
//! the way `GET /auth/session` reads a cookie — this deployment's session key,
//! an active user — with two refusals of its own:
//!
//! - a **token session** (`oxy_auth::token::browser_session`, `kid: tok:…`)
//!   is not a login. Renewing one here would trade a narrow, twelve-hour
//!   credential up to its owner's whole reach;
//! - **no leeway** on expiry: a JWT the request path would refuse a minute
//!   from now is not renewed.
//!
//! `GET /auth/session` itself is no use to the app: it reads only the
//! `oxy_session` cookie, never `Authorization`.
//!
//! Every session handed to the app comes with the cookie that carries it
//! ([`SessionCookie`]), for the app to set in its WebView.

use axum::Json;
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use chrono::{DateTime, Duration, Utc};
use entity::prelude::Users;
use entity::users::{self, UserStatus};
use jsonwebtoken::{Validation, decode};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::session_key::{self, Purpose};
use oxy_auth::token::browser_session;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::Serialize;
use uuid::Uuid;

use super::cookie::SessionCookie;
use super::error::MobileAuthError;
use super::frontline;
use oxy_app_core::session::{Claims, SESSION_TTL_SECS, create_auth_token_with_ttl};

/// The window a mobile sign-in gets: a browser login's thirty days.
pub(super) const LOGIN_WINDOW_SECS: i64 = SESSION_TTL_SECS;

#[derive(Serialize)]
pub struct RefreshResponse {
    pub token: String,
    pub expires_in: i64,
    pub session_cookie: SessionCookie,
}

/// A login session just minted for the app.
pub(super) struct Minted {
    pub(super) token: String,
    /// The JWT's own `exp`.
    pub(super) expires_at: DateTime<Utc>,
}

impl Minted {
    /// Seconds until it lapses, from now.
    pub(super) fn expires_in(&self) -> i64 {
        (self.expires_at - Utc::now()).num_seconds().max(0)
    }
}

/// The claims of `jwt` when it is a live login session signed by this
/// deployment. Reads no database once the key is loaded.
async fn verify(jwt: &str) -> Result<Claims, MobileAuthError> {
    if browser_session::token_id_of(jwt).is_some() {
        return Err(MobileAuthError::InvalidSession);
    }
    let key = session_key::decoding_key(Purpose::Session).await?;
    let mut validation = Validation::default();
    validation.leeway = 0;
    decode::<Claims>(jwt, &key, &validation)
        .map(|data| data.claims)
        .map_err(|e| {
            tracing::debug!("mobile session refused: {e}");
            MobileAuthError::InvalidSession
        })
}

/// The active user `jwt` is a live login session of, and its claims.
async fn signed_in_user(
    db: &DatabaseConnection,
    jwt: &str,
) -> Result<(users::Model, Claims), MobileAuthError> {
    let claims = verify(jwt).await?;
    let user_id = Uuid::parse_str(&claims.sub).map_err(|_| MobileAuthError::InvalidSession)?;
    let user = Users::find_by_id(user_id)
        .one(db)
        .await?
        .filter(|u| u.status == UserStatus::Active)
        .ok_or(MobileAuthError::InvalidSession)?;
    Ok((user, claims))
}

/// Mint a login session for `user` lasting `window`, through the one place a
/// session is created. Its expiry is read back off the JWT, so what the app is
/// told — and the cookie's expiry — is what the token says, to the second.
pub(super) async fn mint(user: &users::Model, window: Duration) -> Result<Minted, MobileAuthError> {
    let token = create_auth_token_with_ttl(user.clone(), window)
        .await
        .map_err(|status| MobileAuthError::Internal(format!("mint a session: {status}")))?;
    let claims = verify(&token)
        .await
        .map_err(|_| MobileAuthError::Internal("a session just minted does not verify".into()))?;
    let expires_at = DateTime::from_timestamp(claims.exp as i64, 0)
        .ok_or_else(|| MobileAuthError::Internal("a session with no expiry".into()))?;
    Ok(Minted { token, expires_at })
}

/// The window a renewal keeps: the one the session was minted with — thirty
/// days for a login, twelve hours for a frontline shift — never longer than a
/// login's.
fn window_of(claims: &Claims) -> Duration {
    let minted_for = claims.exp as i64 - claims.iat as i64;
    Duration::seconds(minted_for.clamp(1, LOGIN_WINDOW_SECS))
}

/// The JWT in `Authorization`, with or without the `Bearer ` scheme — the two
/// forms the request path accepts.
fn presented(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let jwt = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .unwrap_or(raw)
        .trim();
    (!jwt.is_empty()).then_some(jwt)
}

/// `POST /auth/mobile/refresh` — trade a live login session for a fresh one
/// of the same window, with its cookie. An expired one is not renewed: the app
/// signs in again.
///
/// A crew member's shift session is refused like any session it can't renew
/// (`frontline`). The kiosk's PIN login returns that JWT in its body, so
/// without this a crew member could copy it off the kiosk and keep it alive
/// from anywhere by refreshing it every twelve hours.
pub async fn refresh(headers: HeaderMap) -> Result<Json<RefreshResponse>, MobileAuthError> {
    let jwt = presented(&headers).ok_or(MobileAuthError::InvalidSession)?;
    let db = establish_connection().await?;
    let (user, claims) = signed_in_user(&db, jwt).await?;
    let actor = RequestActor::for_user(user.clone().into(), &headers);
    if frontline::frontline_only(&db, &actor).await? {
        tracing::info!(user_id = %user.id, "a crew member's session was not renewed");
        return Err(MobileAuthError::InvalidSession);
    }
    let minted = mint(&user, window_of(&claims)).await?;
    Ok(Json(RefreshResponse {
        expires_in: minted.expires_in(),
        session_cookie: SessionCookie::for_request(&headers, minted.expires_at),
        token: minted.token,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use jsonwebtoken::{EncodingKey, Header, encode};

    /// This process's session key, with a root fixed so no database is read.
    async fn our_key() -> [u8; 32] {
        session_key::install_root_for_tests([11; 32]);
        session_key::key(Purpose::Session).await.expect("a key")
    }

    fn claims(exp_offset_secs: i64, window_secs: i64) -> Claims {
        let now = Utc::now().timestamp();
        Claims {
            sub: Uuid::from_u128(5).to_string(),
            email: "ada@example.com".into(),
            exp: (now + exp_offset_secs) as usize,
            iat: (now + exp_offset_secs - window_secs) as usize,
        }
    }

    fn sign(claims: &Claims, header: Header, key: &[u8]) -> String {
        encode(&header, claims, &EncodingKey::from_secret(key)).expect("encode")
    }

    #[tokio::test]
    async fn a_live_login_session_is_read() {
        let jwt = sign(&claims(3600, 7200), Header::default(), &our_key().await);
        let read = verify(&jwt).await.ok().map(|c| c.sub);
        assert_eq!(read, Some(Uuid::from_u128(5).to_string()));
    }

    #[tokio::test]
    async fn an_expired_or_foreign_session_is_refused() {
        let key = our_key().await;
        // Expired a second ago: no leeway.
        let lapsed = sign(&claims(-1, 3600), Header::default(), &key);
        assert!(matches!(
            verify(&lapsed).await,
            Err(MobileAuthError::InvalidSession)
        ));
        for not_ours in [&b"not-our-key"[..], &b"authentication_secret"[..]] {
            let forged = sign(&claims(3600, 7200), Header::default(), not_ours);
            assert!(matches!(
                verify(&forged).await,
                Err(MobileAuthError::InvalidSession)
            ));
        }
        for garbage in ["", "oxy_pat_abc", "a.b.c"] {
            assert!(matches!(
                verify(garbage).await,
                Err(MobileAuthError::InvalidSession)
            ));
        }
    }

    #[tokio::test]
    async fn a_token_session_is_not_a_login() {
        // Even signed with the login key, a `kid: tok:…` JWT is a session a
        // token opened, and is refused before its signature is read.
        let header = Header {
            kid: Some(format!("tok:{}", Uuid::from_u128(9))),
            ..Header::default()
        };
        let jwt = sign(&claims(3600, 7200), header, &our_key().await);
        assert!(matches!(
            verify(&jwt).await,
            Err(MobileAuthError::InvalidSession)
        ));
    }

    #[test]
    fn a_renewal_keeps_the_window_it_was_minted_with() {
        let shift = 12 * 60 * 60;
        assert_eq!(window_of(&claims(60, shift)).num_seconds(), shift);
        assert_eq!(
            window_of(&claims(60, LOGIN_WINDOW_SECS)).num_seconds(),
            LOGIN_WINDOW_SECS
        );
        // Never longer than a login, whatever the JWT claims.
        let year = 365 * 24 * 60 * 60;
        assert_eq!(
            window_of(&claims(60, year)).num_seconds(),
            LOGIN_WINDOW_SECS
        );
    }

    #[test]
    fn the_session_is_read_from_authorization_with_or_without_bearer() {
        let with = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(AUTHORIZATION, HeaderValue::from_static(value));
            headers
        };
        assert_eq!(presented(&with("Bearer a.b.c")), Some("a.b.c"));
        assert_eq!(presented(&with("bearer a.b.c")), Some("a.b.c"));
        assert_eq!(presented(&with("a.b.c")), Some("a.b.c"));
        assert_eq!(presented(&with("Bearer ")), None);
        assert_eq!(presented(&HeaderMap::new()), None);
    }

    #[tokio::test]
    async fn a_refresh_with_no_session_is_refused_before_anything_is_read() {
        let refused = refresh(HeaderMap::new()).await;
        assert!(matches!(refused, Err(MobileAuthError::InvalidSession)));
    }
}
