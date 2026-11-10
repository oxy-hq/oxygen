//! `POST /api/notifications/test` — send yourself a test notification.
//!
//! A platform notice of kind `oxy.test` to the caller only, through the normal
//! path: an inbox row, then push on the caller's devices (never email or
//! Slack). What it says and the per-person rate limit are
//! [`super::platform::self_test`]; this is the request and the answer.
//!
//! - Body, optional (no body, or `{}`): `{ "org_id": "…" }`, which picks which
//!   of the caller's orgs files it; absent, their oldest membership the
//!   request's token reaches. Nothing else: what the notice says is rendered,
//!   never taken from the caller, because a platform notice stores no text.
//! - `200 { "id" }` — the new inbox row, as `GET /api/notifications` lists it.
//! - `400 { "error": "invalid_body" }` — malformed JSON or an unknown field.
//! - `404` — `org_id` names an org the caller is not in, or that the token
//!   does not reach; `409 { "error": "no_org" }` — the caller is in none.
//! - `429 { "error": "rate_limited", "retry_after" }` plus `Retry-After` —
//!   one per person per 10 seconds, across every replica.

use axum::Json;
use axum::body::Bytes;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::{DatabaseConnection, DbErr};
use serde::Deserialize;
use serde_json::json;
use tracing::instrument;
use uuid::Uuid;

use super::platform::people;
use super::platform::self_test::{TestSent, send_test};

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestRequest {
    pub org_id: Option<Uuid>,
}

/// A refusal with a code the app can branch on.
fn refuse(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

/// `POST /api/notifications/test`
#[instrument(skip_all, fields(user = %user.id))]
pub async fn send_test_notification(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    raw: Bytes,
) -> Response {
    let named_org = match parse(&raw) {
        Ok(req) => req,
        Err(code) => return refuse(StatusCode::BAD_REQUEST, code),
    };
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let org_id = match org_for(&db, &user, named_org).await {
        Ok(Some(org)) => org,
        Ok(None) if named_org.is_some() => return StatusCode::NOT_FOUND.into_response(),
        Ok(None) => return refuse(StatusCode::CONFLICT, "no_org"),
        Err(e) => return internal(e),
    };
    let via = user.credential.as_ref().map(|c| c.token_id);
    match send_test(&db, user.id, org_id, via).await {
        Ok(TestSent::Sent { id }) => Json(json!({ "id": id })).into_response(),
        Ok(TestSent::RateLimited { retry_after_secs }) => rate_limited(retry_after_secs),
        Err(e) => internal(e),
    }
}

fn rate_limited(retry_after: u64) -> Response {
    let body = json!({ "error": "rate_limited", "retry_after": retry_after });
    let mut response = (StatusCode::TOO_MANY_REQUESTS, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(retry_after));
    response
}

fn internal(e: DbErr) -> Response {
    tracing::warn!(error = %e, "test notification failed");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

/// The named org, if any. Read from raw bytes so a call with no body or no
/// `Content-Type` works, while a malformed one — or one carrying a field this
/// route does not take, such as a `body` to show — is a 400 rather than
/// silently ignored.
pub fn parse(raw: &[u8]) -> Result<Option<Uuid>, &'static str> {
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice::<TestRequest>(raw)
        .map(|req| req.org_id)
        .map_err(|_| "invalid_body")
}

/// The org to file the caller's test under: `named` if it is one of theirs,
/// else their oldest (`people::orgs_of`: members first, then frontline
/// standing). Only orgs the request's token reaches, as the inbox lists only
/// those.
pub async fn org_for(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    named: Option<Uuid>,
) -> Result<Option<Uuid>, DbErr> {
    let mut orgs = people::orgs_of(db, user.id)
        .await?
        .into_iter()
        .filter(|org| user.reaches_org(*org));
    Ok(match named {
        Some(want) => orgs.find(|org| *org == want),
        None => orgs.next(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_notice_request_may_be_empty_or_name_an_org() {
        assert_eq!(parse(b""), Ok(None));
        assert_eq!(parse(b"  \n"), Ok(None));
        assert_eq!(parse(b"{}"), Ok(None));
        assert_eq!(parse(br#"{"org_id":null}"#), Ok(None));
        let org = Uuid::new_v4();
        assert_eq!(
            parse(format!(r#"{{"org_id":"{org}"}}"#).as_bytes()),
            Ok(Some(org))
        );
    }

    #[test]
    fn test_notice_request_refuses_malformed_json_and_any_text_to_show() {
        assert_eq!(parse(b"{"), Err("invalid_body"));
        assert_eq!(parse(br#"{"org_id":"not-a-uuid"}"#), Err("invalid_body"));
        for field in ["body", "title", "message"] {
            let raw = format!(r#"{{"{field}":"Hello from Settings"}}"#);
            assert_eq!(parse(raw.as_bytes()), Err("invalid_body"), "{field}");
        }
    }
}
