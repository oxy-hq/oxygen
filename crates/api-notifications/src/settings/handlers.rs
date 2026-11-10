//! `GET` and `PATCH /api/notifications/settings`. Self-scoped: every read and
//! write is the caller's own (`user_id = me`). The document is
//! [`super::document`]; the patch [`super::patch`].
//!
//! The `ETag` is the document's `version`, quoted as HTTP requires
//! (`"7"`); `If-Match` takes it quoted or bare.

use axum::Json;
use axum::body::Bytes;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use serde_json::json;
use tracing::instrument;

use super::document::{NotificationSettings, document_for};
use super::patch::{PatchError, apply};

/// The document with its `ETag`, at `status`.
fn with_etag(status: StatusCode, doc: NotificationSettings) -> Response {
    let etag = HeaderValue::from_str(&format!("\"{}\"", doc.version)).ok();
    let mut response = (status, Json(doc)).into_response();
    if let Some(etag) = etag {
        response.headers_mut().insert(header::ETAG, etag);
    }
    // A cached copy would defeat `If-Match`.
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// `GET /api/notifications/settings`
#[instrument(skip_all, fields(user = %user.id))]
pub async fn get_settings(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    match document_for(&db, &user).await {
        Ok(doc) => with_etag(StatusCode::OK, doc),
        Err(e) => {
            tracing::warn!(error = %e, "notification settings could not be read");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// `PATCH /api/notifications/settings`
#[instrument(skip_all, fields(user = %user.id))]
pub async fn patch_settings(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let if_match = headers.get(header::IF_MATCH).and_then(|v| v.to_str().ok());
    match apply(&db, &user, if_match, &body).await {
        Ok(doc) => with_etag(StatusCode::OK, doc),
        Err(PatchError::Stale(doc)) => with_etag(StatusCode::PRECONDITION_FAILED, *doc),
        Err(PatchError::BadRequest(code)) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": code }))).into_response()
        }
        Err(PatchError::Db(e)) => {
            tracing::warn!(error = %e, "notification settings could not be written");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
