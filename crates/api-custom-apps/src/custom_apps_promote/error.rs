//! What a promote or a rollback refuses with, and how each refusal is
//! answered: `{"error": "<code>", "message": "<sentence>"}` with its status,
//! like every route the sandbox work added (`custom-app-sandboxes.md` §5) —
//! plus `code`, the same value, which is the field `oxyc` reads a server's
//! code from (and the shape `draft_published_by_agent` already has). Two
//! answers carry more: a blocked act also lists its `blockers`, and a build a
//! sandbox agent token published keeps the `409 draft_published_by_agent`
//! body every promote path already answers.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::custom_apps_agent_built::AgentBuilt;
use crate::custom_apps_migrations::MigrationError;

/// One reason the act may not happen yet, as a dry run lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub code: &'static str,
    pub message: String,
}

pub const STAGING_NEVER_SERVED: &str = "staging_never_served";
pub const VALIDATION_NOT_PASSED: &str = "validation_not_passed";
pub const REQUIRED_SECRETS_UNSET: &str = "required_secrets_unset";
pub const SCHEDULE_REMOVAL_UNCONFIRMED: &str = "schedule_removal_unconfirmed";
pub const SAME_PERSON_PROMOTE: &str = "same_person_promote";
pub const EVENT_LISTENERS_BROKEN: &str = "event_listeners_broken";

/// The code a failed migration is answered with, whatever its status.
pub const MIGRATION_FAILED: &str = "migration_failed";

#[derive(Debug, thiserror::Error)]
pub enum PromoteError {
    #[error("{0:?} is not an environment: use production, staging or dev-<handle>")]
    InvalidEnvironment(String),
    #[error(
        "{from} cannot be promoted to {to}: a sandbox (dev-<handle>) promotes to staging, and \
         staging to production — nothing else. Moving production backwards is `oxyc rollback`"
    )]
    PathRefused { from: String, to: String },
    #[error("{0} cannot be rolled back: a sandbox moves only by a publish to it — publish again")]
    NotRollbackable(String),
    #[error(
        "a publish token cannot promote or roll back an app: use a login token (`oxyc login`) or \
         an API key"
    )]
    PublishToken,
    #[error(
        "promoting a sandbox's build is for Oxy staff who may open this app's non-production \
         environments (develop_apps over its org)"
    )]
    NonProduction,
    #[error("app not found")]
    AppNotFound,
    #[error("this app has no environment {0}")]
    EnvironmentNotFound(String),
    #[error("this app has no build {0:?}")]
    BuildNotFound(String),
    #[error("{environment} has never served build {build:?}: name a build it served")]
    NotServed { environment: String, build: String },
    #[error("{0} serves no build: there is nothing to promote")]
    NothingToPromote(String),
    #[error("{0} has served no other build that is still kept: there is nothing to roll back to")]
    NothingToRollBackTo(String),
    #[error("{environment} already serves build {build:?}")]
    AlreadyServing { environment: String, build: String },
    #[error(
        "{environment} moved since the plan was made (expected {expected}, now {actual}): look \
         at the plan again"
    )]
    TargetMoved {
        environment: String,
        expected: String,
        actual: String,
    },
    #[error("{}", blocked_message(.0))]
    Blocked(Vec<Blocker>),
    #[error("{}", .0.message())]
    AgentBuilt(Box<AgentBuilt>),
    /// A build that removes an event type, or changes its schema
    /// incompatibly, while a production listener depends on it: one message
    /// per listener (`app_events::gate`).
    #[error("refused: {}", .0.join("; "))]
    EventListenersBroken(Vec<String>),
    #[error("{0}")]
    Migration(MigrationError),
    #[error("internal error")]
    Internal(String),
}

fn blocked_message(blockers: &[Blocker]) -> String {
    let reasons: Vec<&str> = blockers.iter().map(|b| b.message.as_str()).collect();
    format!("refused: {}", reasons.join("; "))
}

impl PromoteError {
    /// A database or store failure, logged with what was being done.
    pub fn internal(doing: &str, e: impl std::fmt::Display) -> Self {
        tracing::error!("custom-app promote: {doing}: {e}");
        Self::Internal(format!("{doing}: {e}"))
    }

    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidEnvironment(_) | Self::PathRefused { .. } | Self::NotRollbackable(_) => {
                StatusCode::BAD_REQUEST
            }
            Self::PublishToken | Self::NonProduction => StatusCode::FORBIDDEN,
            Self::AppNotFound | Self::EnvironmentNotFound(_) | Self::BuildNotFound(_) => {
                StatusCode::NOT_FOUND
            }
            Self::NotServed { .. }
            | Self::NothingToPromote(_)
            | Self::NothingToRollBackTo(_)
            | Self::AlreadyServing { .. }
            | Self::TargetMoved { .. }
            | Self::Blocked(_)
            | Self::EventListenersBroken(_)
            | Self::AgentBuilt(_) => StatusCode::CONFLICT,
            Self::Migration(e) => migration_status(e),
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidEnvironment(_) => "invalid_environment",
            Self::PathRefused { .. } => "promote_path_refused",
            Self::NotRollbackable(_) => "not_rollbackable",
            Self::PublishToken => "publish_token_refused",
            Self::NonProduction => "non_production_refused",
            Self::AppNotFound => "app_not_found",
            Self::EnvironmentNotFound(_) => "environment_not_found",
            Self::BuildNotFound(_) => "build_not_found",
            Self::NotServed { .. } => "build_not_served",
            Self::NothingToPromote(_) => "nothing_to_promote",
            Self::NothingToRollBackTo(_) => "nothing_to_roll_back_to",
            Self::AlreadyServing { .. } => "already_serving",
            Self::TargetMoved { .. } => "target_moved",
            Self::Blocked(blockers) => blockers.first().map_or("blocked", |b| b.code),
            Self::EventListenersBroken(_) => EVENT_LISTENERS_BROKEN,
            Self::AgentBuilt(_) => crate::custom_apps_agent_built::PROMOTE_REFUSED,
            Self::Migration(_) => MIGRATION_FAILED,
            Self::Internal(_) => "internal",
        }
    }
}

/// A migration's own status: retryable `409`, the author's `422`, ours `500`
/// — the statuses a promoting publish has always answered.
pub fn migration_status(e: &MigrationError) -> StatusCode {
    if e.is_retryable() {
        StatusCode::CONFLICT
    } else if e.is_author_fault() {
        StatusCode::UNPROCESSABLE_ENTITY
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

impl From<MigrationError> for PromoteError {
    fn from(e: MigrationError) -> Self {
        Self::Migration(e)
    }
}

impl From<AgentBuilt> for PromoteError {
    fn from(built: AgentBuilt) -> Self {
        Self::AgentBuilt(Box::new(built))
    }
}

impl IntoResponse for PromoteError {
    fn into_response(self) -> Response {
        let status = self.status();
        match self {
            Self::AgentBuilt(built) => built.into_response(),
            Self::Blocked(ref blockers) => {
                let body = serde_json::json!({
                    "code": self.code(),
                    "error": self.code(),
                    "message": self.to_string(),
                    "blockers": blockers,
                });
                (status, Json(body)).into_response()
            }
            Self::Internal(_) => {
                let body = serde_json::json!({
                    "code": "internal", "error": "internal", "message": "internal error",
                });
                (status, Json(body)).into_response()
            }
            _ => {
                let body = serde_json::json!({
                    "code": self.code(), "error": self.code(), "message": self.to_string(),
                });
                (status, Json(body)).into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One status and one code per refusal, as the contract's table lists them.
    #[test]
    fn every_refusal_has_its_status_and_code() {
        let table = [
            (
                PromoteError::InvalidEnvironment("x".into()),
                400,
                "invalid_environment",
            ),
            (
                PromoteError::PathRefused {
                    from: "production".into(),
                    to: "staging".into(),
                },
                400,
                "promote_path_refused",
            ),
            (
                PromoteError::NotRollbackable("dev-a1".into()),
                400,
                "not_rollbackable",
            ),
            (PromoteError::PublishToken, 403, "publish_token_refused"),
            (PromoteError::NonProduction, 403, "non_production_refused"),
            (PromoteError::AppNotFound, 404, "app_not_found"),
            (
                PromoteError::BuildNotFound("b".into()),
                404,
                "build_not_found",
            ),
            (
                PromoteError::AlreadyServing {
                    environment: "production".into(),
                    build: "b".into(),
                },
                409,
                "already_serving",
            ),
            (
                PromoteError::EventListenersBroken(vec!["x".into()]),
                409,
                "event_listeners_broken",
            ),
            (
                PromoteError::TargetMoved {
                    environment: "production".into(),
                    expected: "a".into(),
                    actual: "b".into(),
                },
                409,
                "target_moved",
            ),
        ];
        for (refusal, status, code) in table {
            assert_eq!(refusal.status().as_u16(), status, "{refusal:?}");
            assert_eq!(refusal.code(), code, "{refusal:?}");
        }
    }

    /// A blocked act answers its first blocker's code and lists them all.
    #[tokio::test]
    async fn a_blocked_act_lists_every_blocker() {
        let refused = PromoteError::Blocked(vec![
            Blocker {
                code: STAGING_NEVER_SERVED,
                message: "staging never served it".into(),
            },
            Blocker {
                code: REQUIRED_SECRETS_UNSET,
                message: "STRIPE_KEY is unset".into(),
            },
        ]);
        let response = refused.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("a body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        assert_eq!(body["error"], STAGING_NEVER_SERVED);
        assert_eq!(body["blockers"][1]["code"], REQUIRED_SECRETS_UNSET);
        let message = body["message"].as_str().unwrap_or_default();
        assert!(message.contains("STRIPE_KEY"), "{message}");
    }

    #[test]
    fn a_migration_keeps_its_own_status() {
        let busy = PromoteError::Migration(MigrationError::Busy);
        assert_eq!(busy.status(), StatusCode::CONFLICT);
        let edited = PromoteError::Migration(MigrationError::ChecksumMismatch {
            filename: "0001.sql".into(),
            applied: "a".into(),
            bundled: "b".into(),
        });
        assert_eq!(edited.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(edited.code(), MIGRATION_FAILED);
    }
}
