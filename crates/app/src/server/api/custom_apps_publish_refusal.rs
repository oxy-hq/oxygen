//! What `POST /api/customer-apps/publish` answers a refusal with.
//!
//! Plain text, as the route always has: `oxyc` prints the body. One refusal
//! is different: a client that holds a sandbox agent token branches on it, so
//! it is JSON with a `code` (`PublishError::code`). `error` repeats the code
//! for a client that reads that field, as the sandbox routes name theirs.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::custom_apps_publish::PublishError;

/// A refused publish: its status, and a text or a coded JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRefusal {
    status: StatusCode,
    /// `Some` answers JSON.
    code: Option<&'static str>,
    message: String,
}

impl PublishRefusal {
    /// A refusal answered as plain text.
    pub fn text(status: StatusCode, message: String) -> Self {
        Self {
            status,
            code: None,
            message,
        }
    }
}

impl From<(StatusCode, String)> for PublishRefusal {
    fn from((status, message): (StatusCode, String)) -> Self {
        Self::text(status, message)
    }
}

impl From<PublishError> for PublishRefusal {
    fn from(refused: PublishError) -> Self {
        Self {
            status: refused.status(),
            code: refused.code(),
            message: refused.to_string(),
        }
    }
}

impl IntoResponse for PublishRefusal {
    fn into_response(self) -> Response {
        match self.code {
            Some(code) => {
                let body = serde_json::json!({
                    "code": code,
                    "error": code,
                    "message": self.message,
                });
                (self.status, Json(body)).into_response()
            }
            None => (self.status, self.message).into_response(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn body_of(refusal: PublishRefusal) -> (StatusCode, Option<String>, String) {
        let response = refusal.into_response();
        let status = response.status();
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("read the body");
        (status, content_type, String::from_utf8_lossy(&bytes).into())
    }

    /// The refusal a sandbox agent token's client branches on is JSON, with
    /// the code under both names it may be read by.
    #[tokio::test]
    async fn a_sandbox_token_refusal_is_json_with_a_code() {
        let (status, content_type, body) = body_of(PublishError::SandboxTokenRefused.into()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(content_type.as_deref(), Some("application/json"));
        let body: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
        assert_eq!(body["code"], "sandbox_token_refused");
        assert_eq!(body["error"], "sandbox_token_refused");
        assert!(body["message"].as_str().is_some_and(|m| !m.is_empty()));
    }

    /// Every other refusal is the text it always was.
    #[tokio::test]
    async fn every_other_refusal_stays_plain_text() {
        let refused = PublishError::SandboxRefused;
        let text = refused.to_string();
        let (status, content_type, body) = body_of(refused.into()).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(content_type.is_some_and(|t| t.starts_with("text/plain")));
        assert_eq!(body, text);

        let (status, _, body) =
            body_of((StatusCode::BAD_REQUEST, "missing app".to_string()).into()).await;
        assert_eq!(
            (status, body.as_str()),
            (StatusCode::BAD_REQUEST, "missing app")
        );
    }
}
