//! What a mobile sign-in route answers when it refuses: `{ "error", "code" }`,
//! the shape the token routes answer in.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oxy_shared::errors::OxyError;
use serde_json::json;

#[derive(Debug)]
pub enum MobileAuthError {
    /// 400 `invalid_request` — a body that cannot be read, or a challenge
    /// that is not S256.
    InvalidRequest(String),
    /// 400 `invalid_redirect_uri` — not an `oxygen://` URI.
    InvalidRedirectUri,
    /// 400 `invalid_code` — an exchange that cannot be honoured: unknown,
    /// spent or expired, a verifier or redirect URI that does not match, or a
    /// user gone since. One answer for all, so a caller cannot tell which.
    InvalidCode,
    /// 401 `invalid_session` — not a live login session of an active user: a
    /// token session, an expired or foreign-signed JWT, or none at all.
    InvalidSession,
    /// 403 `frontline_not_supported` — a crew member's session asked to sign
    /// the app in. Crew sign-in stays on the store's enrolled device.
    FrontlineNotSupported,
    /// 500 — logged, never described.
    Internal(String),
}

impl MobileAuthError {
    fn parts(&self) -> (StatusCode, String, &'static str) {
        match self {
            Self::InvalidRequest(message) => {
                (StatusCode::BAD_REQUEST, message.clone(), "invalid_request")
            }
            Self::InvalidRedirectUri => (
                StatusCode::BAD_REQUEST,
                "'redirect_uri' must be an oxygen:// URI with no query or fragment".into(),
                "invalid_redirect_uri",
            ),
            Self::InvalidCode => (
                StatusCode::BAD_REQUEST,
                "the code is invalid, expired or already used".into(),
                "invalid_code",
            ),
            Self::InvalidSession => (
                StatusCode::UNAUTHORIZED,
                "the session is invalid or expired; sign in again".into(),
                "invalid_session",
            ),
            Self::FrontlineNotSupported => (
                StatusCode::FORBIDDEN,
                "Crew sign-in works only on a store's enrolled device.".into(),
                "frontline_not_supported",
            ),
            Self::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".into(),
                "internal",
            ),
        }
    }
}

impl IntoResponse for MobileAuthError {
    fn into_response(self) -> Response {
        if let Self::Internal(detail) = &self {
            tracing::error!(error = %detail, "mobile sign-in request failed");
        }
        let (status, message, code) = self.parts();
        (status, Json(json!({ "error": message, "code": code }))).into_response()
    }
}

impl From<OxyError> for MobileAuthError {
    fn from(e: OxyError) -> Self {
        Self::Internal(e.to_string())
    }
}

impl From<sea_orm::DbErr> for MobileAuthError {
    fn from(e: sea_orm::DbErr) -> Self {
        Self::Internal(format!("database: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn answered(e: MobileAuthError) -> (StatusCode, serde_json::Value) {
        let response = e.into_response();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (status, serde_json::from_slice(&bytes).expect("json"))
    }

    #[tokio::test]
    async fn each_refusal_answers_its_contract_status_and_code() {
        for (error, status, code) in [
            (
                MobileAuthError::InvalidRequest("bad".into()),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                MobileAuthError::InvalidRedirectUri,
                StatusCode::BAD_REQUEST,
                "invalid_redirect_uri",
            ),
            (
                MobileAuthError::InvalidCode,
                StatusCode::BAD_REQUEST,
                "invalid_code",
            ),
            (
                MobileAuthError::InvalidSession,
                StatusCode::UNAUTHORIZED,
                "invalid_session",
            ),
            (
                MobileAuthError::FrontlineNotSupported,
                StatusCode::FORBIDDEN,
                "frontline_not_supported",
            ),
        ] {
            let (answered_status, body) = answered(error).await;
            assert_eq!(answered_status, status, "{code}");
            assert_eq!(body["code"], code);
            assert!(body["error"].is_string(), "{code}");
        }
    }

    #[tokio::test]
    async fn an_internal_failure_is_never_described() {
        let (status, body) = answered(MobileAuthError::Internal("db password=x".into())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body.to_string().contains("password"));
    }
}
