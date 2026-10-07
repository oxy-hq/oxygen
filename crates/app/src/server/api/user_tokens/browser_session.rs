//! A browser session for a personal access token, over HTTP (the ticket and
//! the session live in `oxy_auth::token::browser_session`; read that first).
//!
//! An agent holding a token cannot get through Google or a magic-link inbox,
//! so on a deployment it had no way to a signed-in browser. Two routes, on
//! two sides of the auth gate, as `oxyc login`'s are — pointed the other way:
//!
//! - `POST /auth/browser-ticket` — the **token** asks for a one-time ticket.
//!   Five minutes, single use. `oxyc login-link` turns it into a URL.
//! - `POST /auth/browser-ticket/redeem` — **public**. A browser trades the
//!   ticket for a session and its cookie, in the shape every login answers
//!   in. The ticket is the only thing that names the token, so every failure
//!   answers the same 400 `invalid_ticket`.
//!
//! **No authority is minted here.** The session authenticates as the token it
//! came from: its grants, its ceilings, its 403 on every session-only route.
//! It ends when the token does, and no later than twelve hours.
//!
//! Who may ask is narrow on purpose:
//!
//! - only a **personal** token. A service account is not a person to sign in
//!   as, a CI token lives fifteen minutes, a sandbox agent token is confined
//!   to its apps' sandbox loop, and a legacy key gains nothing it did not
//!   have (API-tokens design §3.5);
//! - only the token **presented as a token**. A session it opened carries the
//!   same credential, and would otherwise renew itself for as long as the
//!   token lives — which is the one thing the twelve hours are for.

use axum::Json;
use axum::body::Bytes;
use axum::http::HeaderMap;
use axum::http::header::CONTENT_TYPE;
use chrono::{DateTime, Utc};
use entity::prelude::{ApiTokens, Users};
use entity::users::UserStatus;
use entity::{api_tokens, users};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::browser_session::{self, SESSION_TTL_SECS, Session};
use oxy_auth::token::{
    CredentialContext, StoredKind, authenticate_browser_session, presents_api_token,
};
use oxy_auth::types::AuthenticatedUser;
use oxy_shared::errors::OxyError;
use sea_orm::{DatabaseConnection, EntityTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::audit::{self, Event};
use super::error::TokenError;
use super::handlers::parse;
use super::service;
use crate::server::api::auth::{AuthResponse, token_session_response};

/// The web app's page that redeems a ticket. The ticket rides in the URL
/// **fragment**, which a browser never sends: it reaches no access log and no
/// `Referer`.
const REDEEM_PAGE: &str = "/token-login";

#[derive(Debug, Serialize)]
pub struct TicketResponse {
    pub ticket: String,
    /// Where a browser redeems it, relative to the deployment's origin.
    pub path: String,
    pub expires_at: DateTime<Utc>,
    /// The longest the session it opens will last.
    pub session_seconds: i64,
}

#[derive(Deserialize)]
pub struct RedeemBody {
    ticket: String,
}

/// The personal token this request presented, or why it may not ask.
fn asking_token<'a>(
    headers: &HeaderMap,
    actor: &'a RequestActor,
) -> Result<&'a CredentialContext, TokenError> {
    let credential = actor.credential.as_ref().ok_or(TokenError::NoToken)?;
    if credential.kind != StoredKind::Personal || credential.is_legacy() {
        return Err(TokenError::PersonalTokenRequired);
    }
    // A token session carries its token's credential. It presents a JWT, not
    // the token, and that is what tells the two apart.
    if !presents_api_token(headers) {
        return Err(TokenError::NoToken);
    }
    Ok(credential)
}

/// Issue a one-time ticket that signs a browser in as the calling token
pub async fn issue_ticket(
    headers: HeaderMap,
    actor: RequestActor,
) -> Result<Json<TicketResponse>, TokenError> {
    let credential = asking_token(&headers, &actor)?;
    let db = establish_connection().await?;
    let issued = browser_session::issue(&db, actor.id, credential.token_id).await?;
    Ok(Json(TicketResponse {
        path: format!("{REDEEM_PAGE}#ticket={}", issued.ticket),
        ticket: issued.ticket,
        expires_at: issued.expires_at,
        session_seconds: SESSION_TTL_SECS,
    }))
}

/// This route answers with a session cookie, so it takes a JSON body and
/// nothing looser. A page on another site can post a form here — and
/// `text/plain` can be shaped into JSON — which would set the cookie of a
/// session **its author** chose in a visitor's browser. `application/json` is
/// the content type a cross-site page cannot send without the CORS preflight
/// that refuses it; every other login route gets this from its extractor.
fn require_json(headers: &HeaderMap) -> Result<(), TokenError> {
    let json = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"));
    if json {
        return Ok(());
    }
    Err(TokenError::Invalid(
        "the body must be application/json".into(),
    ))
}

/// Redeem a ticket for a browser session of the token that asked for it
pub async fn redeem_ticket(
    headers: HeaderMap,
    body: Bytes,
) -> Result<(HeaderMap, Json<AuthResponse>), TokenError> {
    // Before the ticket is read: a refused request must not spend it.
    require_json(&headers)?;
    let body: RedeemBody = parse(&body)?;
    let db = establish_connection().await?;
    let token_id = browser_session::redeem(&db, body.ticket.trim())
        .await?
        .ok_or(TokenError::InvalidTicket)?;
    let (row, user) = ticketed(&db, token_id).await?;

    let now = Utc::now();
    let session = browser_session::mint(&row, user.email.as_deref().unwrap_or(""), now)?;
    let credential = admitted(&session).await?;
    record_opened(&db, &row, &user, &credential, &session, &headers).await?;

    let max_age = Some(session.max_age_secs(now));
    token_session_response(&headers, &user, &credential, session.jwt, max_age, &db)
        .await
        .map_err(|status| TokenError::Internal(format!("token session payload: {status}")))
}

/// The token a ticket named and the user it acts as. Either gone since the
/// ticket was issued redeems nothing.
async fn ticketed(
    db: &DatabaseConnection,
    token_id: uuid::Uuid,
) -> Result<(api_tokens::Model, users::Model), TokenError> {
    let row = ApiTokens::find_by_id(token_id)
        .one(db)
        .await?
        .ok_or(TokenError::InvalidTicket)?;
    let user = Users::find_by_id(row.principal_user_id)
        .one(db)
        .await?
        .filter(|u| u.status == UserStatus::Active)
        .ok_or(TokenError::InvalidTicket)?;
    Ok((row, user))
}

/// The credential the new session authenticates as, by the check every later
/// request of it gets — so a token revoked, expired, narrowed to nothing or
/// regenerated since the ticket was issued opens no session, and what is
/// handed over is known to work.
async fn admitted(session: &Session) -> Result<CredentialContext, TokenError> {
    match authenticate_browser_session(&session.jwt).await {
        Ok((_, credential)) => Ok(credential),
        Err(OxyError::AuthenticationError(reason)) => {
            tracing::info!(%reason, "a ticket's token no longer opens a session");
            Err(TokenError::InvalidTicket)
        }
        Err(other) => Err(other.into()),
    }
}

/// One audit event per session opened, on the token and naming it as the
/// actor: where a credential became a browser, and from what address.
async fn record_opened(
    db: &DatabaseConnection,
    row: &api_tokens::Model,
    user: &users::Model,
    credential: &CredentialContext,
    session: &Session,
    headers: &HeaderMap,
) -> Result<(), TokenError> {
    let mut actor = RequestActor::for_user(AuthenticatedUser::from(user.clone()), headers);
    actor.user = actor.user.with_credential(Some(credential.clone()));
    actor.credential = Some(credential.clone());
    Event {
        action: audit::BROWSER_SESSION_OPENED,
        token: row,
        orgs: service::reach_of(db, row).await?,
        detail: json!({ "session_expires_at": session.expires_at.to_rfc3339() }),
        change: None,
    }
    .record(db, &actor)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use uuid::Uuid;

    fn credential(kind: StoredKind, legacy_api_key_id: Option<Uuid>) -> CredentialContext {
        CredentialContext {
            token_id: Uuid::from_u128(9),
            kind,
            principal_user_id: Uuid::from_u128(1),
            all_access: true,
            platform: false,
            partner: false,
            name: "t".into(),
            display_prefix: "oxy_pat_".into(),
            legacy_api_key_id,
            blocked_orgs: Vec::new(),
            expires_at: None,
            grants: Vec::new(),
            app_publish: Vec::new(),
            app_sandbox: Vec::new(),
            service_account: None,
        }
    }

    fn actor(credential: Option<CredentialContext>) -> RequestActor {
        let mut actor = RequestActor::session(AuthenticatedUser {
            id: Uuid::from_u128(1),
            email: Some("ada@example.com".into()),
            name: "Ada".into(),
            picture: None,
            status: UserStatus::Active,
            credential: None,
        });
        actor.credential = credential;
        actor
    }

    fn presenting(value: &'static str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static(value));
        headers
    }

    #[test]
    fn only_a_json_body_is_redeemed() {
        let with = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(value));
            headers
        };
        for json in ["application/json", "Application/JSON; charset=utf-8"] {
            assert!(require_json(&with(json)).is_ok(), "{json}");
        }
        // What a cross-site form can send, and no content type at all.
        for other in [
            "text/plain",
            "application/x-www-form-urlencoded",
            "multipart/form-data; boundary=x",
            "application/jsonp",
        ] {
            assert!(require_json(&with(other)).is_err(), "{other}");
        }
        assert!(require_json(&HeaderMap::new()).is_err());
    }

    #[test]
    fn a_personal_token_presented_as_a_token_may_ask() {
        let actor = actor(Some(credential(StoredKind::Personal, None)));
        let asked = asking_token(&presenting("Bearer oxy_pat_abc"), &actor);
        assert_eq!(asked.map(|c| c.token_id).ok(), Some(Uuid::from_u128(9)));
    }

    #[test]
    fn a_login_session_has_no_token_to_ask_with() {
        let session = actor(None);
        let refused = asking_token(&presenting("eyJ.login.jwt"), &session);
        assert!(matches!(refused, Err(TokenError::NoToken)));
    }

    #[test]
    fn a_token_session_cannot_renew_itself() {
        // The credential is the token's, but what was presented is the
        // session's JWT (here in the header; the cookie reads the same).
        let actor = actor(Some(credential(StoredKind::Personal, None)));
        for headers in [presenting("eyJ.token.session"), HeaderMap::new()] {
            assert!(matches!(
                asking_token(&headers, &actor),
                Err(TokenError::NoToken)
            ));
        }
    }

    #[test]
    fn every_other_kind_of_credential_is_refused() {
        let legacy_minted = credential(StoredKind::Personal, Some(Uuid::from_u128(4)));
        let others = [
            credential(StoredKind::LegacyKey, Some(Uuid::from_u128(4))),
            credential(StoredKind::ServiceAccount, None),
            credential(StoredKind::Ci, None),
            credential(StoredKind::SandboxAgent, None),
            legacy_minted,
        ];
        for other in others {
            let kind = other.kind;
            let actor = actor(Some(other));
            assert!(
                matches!(
                    asking_token(&presenting("Bearer oxy_pat_abc"), &actor),
                    Err(TokenError::PersonalTokenRequired)
                ),
                "{kind:?}"
            );
        }
    }
}
