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
//!
//! * A promoting publish production refused (`409`) is JSON for everyone, in
//!   the body a refused `oxyc promote` answers: the first blocker's code
//!   under `code` and `error`, the sentence, and every blocker under
//!   `blockers` (`custom_apps_publish::production_gate`).
//!
//! A refusal from inside the pipeline names the stage it came from in the
//! [`STAGE_HEADER`] header, whatever the body: `oxyc` prints it, and the body
//! every client already reads is unchanged.

use axum::Json;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

use super::custom_apps_agent_body::Refusal;
use super::custom_apps_promote::Blocker;
use super::custom_apps_publish::PublishError;
use super::custom_apps_publish_attempts::Stage;

/// The response header naming the pipeline stage a publish was refused at.
pub const STAGE_HEADER: &str = "x-oxy-publish-stage";

/// A refused publish: its status, and a text or a coded JSON body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishRefusal {
    status: StatusCode,
    /// `Some` answers JSON.
    code: Option<&'static str>,
    message: String,
    /// The pipeline stage it was refused at, sent as [`STAGE_HEADER`].
    stage: Option<Stage>,
    /// What a refused promoting publish lists; empty for every other refusal.
    blockers: Vec<Blocker>,
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
            stage: None,
            blockers: Vec::new(),
        }
    }

    /// `refused`, as its publisher is told it. `agent` is whether that is a
    /// sandbox agent token; for anyone else this is the `From` below.
    pub fn told(refused: PublishError, agent: bool) -> Self {
        let for_agent = || agent_code(&refused).filter(|_| agent);
        let (status, code, message) = (
            refused.status(),
            refused.code().or_else(for_agent),
            refused.to_string(),
        );
        let blockers = match refused {
            PublishError::ProductionBlocked(blockers) => blockers,
            _ => Vec::new(),
        };
        Self {
            status,
            code,
            message,
            stage: None,
            blockers,
        }
    }

    /// The same refusal, naming the pipeline stage it came from.
    pub fn at_stage(self, stage: Option<Stage>) -> Self {
        Self { stage, ..self }
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
        let mut response = match self.code {
            Some(code) if !self.blockers.is_empty() => {
                let body = serde_json::json!({
                    "code": code,
                    "error": code,
                    "message": self.message,
                    "blockers": self.blockers,
                });
                (self.status, Json(body)).into_response()
            }
            Some(code) => Refusal::new(self.status, code, self.message).into_response(),
            None => (self.status, self.message).into_response(),
        };
        if let Some(stage) = self.stage {
            response
                .headers_mut()
                .insert(STAGE_HEADER, HeaderValue::from_static(stage.name()));
        }
        response
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

    /// A promoting publish production refused: a 409 in the body a refused
    /// `oxyc promote` answers, naming every blocker, whoever publishes.
    #[tokio::test]
    async fn a_refused_promoting_publish_lists_every_blocker_as_json() {
        use crate::custom_apps_promote::blockers::same_person_promote;
        use crate::custom_apps_promote::error::{REQUIRED_SECRETS_UNSET, SAME_PERSON_PROMOTE};
        let refused = || {
            PublishError::ProductionBlocked(vec![
                Blocker {
                    code: REQUIRED_SECRETS_UNSET,
                    message: "the build declares required keys production has no value for: \
                              STRIPE_KEY"
                        .into(),
                },
                same_person_promote("b-7"),
            ])
        };
        for agent in [false, true] {
            let (status, content_type, body) =
                body_of(PublishRefusal::told(refused(), agent)).await;
            assert_eq!(status, StatusCode::CONFLICT);
            assert_eq!(content_type.as_deref(), Some("application/json"));
            let body: serde_json::Value = serde_json::from_str(&body).expect("a JSON body");
            assert_eq!(body["code"], REQUIRED_SECRETS_UNSET);
            assert_eq!(body["error"], REQUIRED_SECRETS_UNSET);
            assert_eq!(body["blockers"][0]["code"], REQUIRED_SECRETS_UNSET);
            assert_eq!(body["blockers"][1]["code"], SAME_PERSON_PROMOTE);
            let message = body["message"].as_str().unwrap_or_default();
            assert!(message.contains("STRIPE_KEY"), "{message}");
            assert!(message.contains("nothing of it was published"), "{message}");
        }
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

    /// A refusal from inside the pipeline names its stage in a header; its
    /// status and body are what they were. One from outside names none.
    #[tokio::test]
    async fn a_staged_refusal_names_its_stage_in_a_header() {
        let refused = || PublishError::BadTarball("not gzip".into());
        let text = refused().to_string();
        let response = PublishRefusal::from(refused())
            .at_stage(Some(Stage::Inflate))
            .into_response();
        assert_eq!(
            response.headers().get(STAGE_HEADER).map(|v| v.as_bytes()),
            Some(&b"inflate"[..])
        );
        let (status, _, body) =
            body_of(PublishRefusal::from(refused()).at_stage(Some(Stage::Inflate))).await;
        assert_eq!((status, body), (StatusCode::UNPROCESSABLE_ENTITY, text));

        let unstaged = PublishRefusal::from(refused())
            .at_stage(None)
            .into_response();
        assert!(unstaged.headers().get(STAGE_HEADER).is_none());
    }
}
