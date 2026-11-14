//! The PKCE pair: the browser approves (`authorize`), the app redeems
//! (`exchange`).

use axum::Json;
use axum::body::Bytes;
use axum::http::HeaderMap;
use chrono::Duration;
use entity::prelude::Users;
use entity::users::{self, UserStatus};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::SessionOnly;
use oxy_auth::token::{cli_login, mobile_login};
use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ApproveMobileSignIn;
use super::cookie::SessionCookie;
use super::error::MobileAuthError;
use super::{frontline, session};

/// The only PKCE method accepted: `plain` would put the verifier in the URL.
const S256: &str = "S256";

#[derive(Deserialize)]
pub struct AuthorizeBody {
    code_challenge: String,
    code_challenge_method: String,
    redirect_uri: String,
}

#[derive(Serialize)]
pub struct AuthorizeResponse {
    pub code: String,
}

#[derive(Deserialize)]
pub struct ExchangeBody {
    code: String,
    code_verifier: String,
    redirect_uri: String,
}

#[derive(Serialize)]
pub struct ExchangeResponse {
    /// A login session JWT: send it as `Authorization: Bearer <token>`.
    pub token: String,
    /// Seconds until it lapses. `POST /api/auth/mobile/refresh` renews it.
    pub expires_in: i64,
    pub user: MobileUser,
    /// The `oxy_session` cookie carrying `token`, for the app to set in its
    /// WebView before it opens a custom app.
    pub session_cookie: SessionCookie,
}

#[derive(Serialize)]
pub struct MobileUser {
    pub id: Uuid,
    /// `null` for a frontline worker, who has no address.
    pub email: Option<String>,
    pub name: String,
}

impl From<&users::Model> for MobileUser {
    fn from(user: &users::Model) -> Self {
        Self {
            id: user.id,
            email: user.email.clone(),
            name: user.name.clone(),
        }
    }
}

/// `POST /auth/mobile/authorize` — issue a one-time code for the session's
/// user, bound to the app's challenge and redirect URI. Five minutes, single
/// use. The refusal is said here: the exchange can only answer `invalid_code`.
/// A crew member's session is refused (`frontline`).
pub async fn authorize(
    _: SessionOnly<ApproveMobileSignIn>,
    actor: RequestActor,
    body: Bytes,
) -> Result<Json<AuthorizeResponse>, MobileAuthError> {
    let body: AuthorizeBody = serde_json::from_slice(&body)
        .map_err(|e| MobileAuthError::InvalidRequest(format!("invalid body: {e}")))?;
    let (challenge, redirect_uri) = checked(&body)?;
    let db = establish_connection().await?;
    if frontline::frontline_only(&db, &actor).await? {
        return Err(MobileAuthError::FrontlineNotSupported);
    }
    let code = mobile_login::authorize(&db, actor.id, &challenge, &redirect_uri).await?;
    Ok(Json(AuthorizeResponse { code }))
}

/// The challenge and redirect URI as they are stored, or why not.
fn checked(body: &AuthorizeBody) -> Result<(String, String), MobileAuthError> {
    if body.code_challenge_method.trim() != S256 {
        return Err(MobileAuthError::InvalidRequest(
            "'code_challenge_method' must be S256".into(),
        ));
    }
    let challenge = cli_login::clean_challenge(&body.code_challenge).ok_or_else(|| {
        MobileAuthError::InvalidRequest(
            "'code_challenge' must be a base64url S256 challenge".into(),
        )
    })?;
    let redirect_uri = mobile_login::clean_redirect_uri(&body.redirect_uri)
        .ok_or(MobileAuthError::InvalidRedirectUri)?;
    Ok((challenge, redirect_uri))
}

/// `POST /auth/mobile/exchange` — redeem the code for a login session of the
/// user it was issued to. A body that cannot be read is a code that cannot be
/// redeemed: every refusal is the same 400 `invalid_code`.
pub async fn exchange(
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<ExchangeResponse>, MobileAuthError> {
    let body: ExchangeBody =
        serde_json::from_slice(&body).map_err(|_| MobileAuthError::InvalidCode)?;
    let db = establish_connection().await?;
    let user_id = mobile_login::redeem(&db, &body.code, &body.code_verifier, &body.redirect_uri)
        .await?
        .ok_or(MobileAuthError::InvalidCode)?;
    let user = Users::find_by_id(user_id)
        .one(&db)
        .await?
        .filter(|u| u.status == UserStatus::Active)
        .ok_or(MobileAuthError::InvalidCode)?;
    // A code a crew session approved before `authorize` refused them.
    let redeemer = RequestActor::for_user(user.clone().into(), &headers);
    if frontline::frontline_only(&db, &redeemer).await? {
        tracing::info!(user_id = %user.id, "a crew member's mobile code was not honoured");
        return Err(MobileAuthError::InvalidCode);
    }
    let window = Duration::seconds(session::LOGIN_WINDOW_SECS);
    let minted = session::mint(&user, window).await?;
    tracing::info!(user_id = %user.id, "mobile app signed in");
    Ok(Json(ExchangeResponse {
        expires_in: minted.expires_in(),
        user: MobileUser::from(&user),
        session_cookie: SessionCookie::for_request(&headers, minted.expires_at),
        token: minted.token,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    fn body(method: &str, challenge: &str, redirect_uri: &str) -> AuthorizeBody {
        AuthorizeBody {
            code_challenge: challenge.into(),
            code_challenge_method: method.into(),
            redirect_uri: redirect_uri.into(),
        }
    }

    #[test]
    fn a_mobile_s256_request_is_checked_into_what_is_stored() {
        let stored = checked(&body("S256", &format!("{CHALLENGE}="), " oxygen://auth "));
        let (challenge, redirect) = stored.expect("a valid request");
        assert_eq!(challenge, CHALLENGE, "padding dropped, as the CLI's");
        assert_eq!(redirect, "oxygen://auth");
    }

    #[test]
    fn only_s256_is_accepted() {
        for method in ["plain", "s256", ""] {
            assert!(
                matches!(
                    checked(&body(method, CHALLENGE, "oxygen://auth")),
                    Err(MobileAuthError::InvalidRequest(_))
                ),
                "{method:?}"
            );
        }
    }

    #[test]
    fn a_malformed_challenge_is_an_invalid_request() {
        assert!(matches!(
            checked(&body("S256", "short", "oxygen://auth")),
            Err(MobileAuthError::InvalidRequest(_))
        ));
    }

    #[test]
    fn a_redirect_uri_off_the_mobile_scheme_is_refused_by_name() {
        for uri in [
            "https://evil.example.com/cb",
            "exp://192.168.1.4:8081/--/auth",
            "exp+oxygen://auth",
            "oxygen://auth?x=1",
        ] {
            assert!(
                matches!(
                    checked(&body("S256", CHALLENGE, uri)),
                    Err(MobileAuthError::InvalidRedirectUri)
                ),
                "{uri:?}"
            );
        }
    }

    #[tokio::test]
    async fn an_unreadable_exchange_is_an_invalid_code() {
        // Refused before any database is touched: the same 400 as a bad code.
        for raw in [&b"not json"[..], br#"{"code":"c"}"#, b""] {
            let refused = exchange(HeaderMap::new(), Bytes::from_static(raw)).await;
            assert!(
                matches!(refused, Err(MobileAuthError::InvalidCode)),
                "{}",
                String::from_utf8_lossy(raw)
            );
        }
    }
}
