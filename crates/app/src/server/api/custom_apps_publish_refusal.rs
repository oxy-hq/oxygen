//! What `POST /api/customer-apps/publish` answers a refusal with.
//!
//! Plain text, as the route always has: `oxyc` prints the body. A refusal
//! that carries a code is the exception. It is answered in the one body a
//! sandbox agent token is refused with (`custom_apps_agent_body`): JSON, the
//! code under `code` and under `error`.
//!
//! * `sandbox_token_refused` is coded for whoever gets it
//!   (`PublishError::code`): only the token can, and its client branches on
//!   it.
//! * A sandbox that is not there is coded **for the token alone**
//!   ([`PublishRefusal::told`]): `environment_not_found`, the code its other
//!   routes give the same refusal. Every other caller reads it as text.
//!
//! The token's other refusals leave here as text and are named after their
//! status on the way out (`custom_apps_agent_body::shape_for_agent`).

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::custom_apps_agent_body::Refusal;
use super::custom_apps_publish::PublishError;

/// A refused publish: its status, and a text or a coded JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRefusal {
    status: StatusCode,
    /// `Some` answers JSON.
    code: Option<&'static str>,
    message: String,
}

/// The code a sandbox agent token is given for a refusal that is text for
/// every other caller.
fn agent_code(refused: &PublishError) -> Option<&'static str> {
    match refused {
        PublishError::UnknownEnvironment { .. } => Some("environment_not_found"),
        _ => None,
    }
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

    /// `refused`, as its publisher is told it. `agent` is whether that is a
    /// sandbox agent token; for anyone else this is the `From` below.
    pub fn told(refused: PublishError, agent: bool) -> Self {
        let for_agent = || agent_code(&refused).filter(|_| agent);
        Self {
            status: refused.status(),
            code: refused.code().or_else(for_agent),
            message: refused.to_string(),
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
        Self::told(refused, false)
    }
}

impl IntoResponse for PublishRefusal {
    fn into_response(self) -> Response {
        match self.code {
            Some(code) => Refusal::new(self.status, code, self.message).into_response(),
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

    /// A sandbox that is not there: `environment_not_found` as JSON for a
    /// sandbox agent token, and for anyone else the text it always was.
    #[tokio::test]
    async fn a_missing_sandbox_is_coded_for_the_token_alone() {
        let missing = || PublishError::UnknownEnvironment {
            name: "dev-a".into(),
        };
        let text = missing().to_string();

        let (status, content_type, body) = body_of(PublishRefusal::told(missing(), true)).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(content_type.as_deref(), Some("application/json"));
        let body: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
        assert_eq!(body["code"], "environment_not_found");
        assert_eq!(body["error"], "environment_not_found");
        assert_eq!(body["message"], text.as_str());

        for refusal in [PublishRefusal::told(missing(), false), missing().into()] {
            let (status, content_type, body) = body_of(refusal).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert!(content_type.is_some_and(|t| t.starts_with("text/plain")));
            assert_eq!(body, text);
        }
        // Being the token codes nothing else: a bundle's refusal stays text.
        let refused = PublishRefusal::told(PublishError::SandboxWithPromote, true);
        let (_, content_type, _) = body_of(refused).await;
        assert!(content_type.is_some_and(|t| t.starts_with("text/plain")));
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
