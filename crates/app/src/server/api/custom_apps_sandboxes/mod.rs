//! Sandbox environments of a custom app: `dev-<handle>` rows of
//! `app_environments`, created and deleted explicitly, and deleted on their
//! own after a week idle (`internal-docs/custom-app-sandboxes.md`).
//!
//! A sandbox is the environments design's dev slot made real: its own build
//! pointer and its own homes for writes (a storage silo, secrets, an Airhouse
//! sibling, a schema on the org's OLTP staging branch), under the per-call
//! policy staging has. Everything that *runs* in
//! one — resolution, admission, the policy, the homes — lives where staging's
//! does and treats a sandbox as one more non-production environment. This
//! module owns only what staging never needed: the row's lifecycle.
//!
//! * here — the limits, the [`EnvironmentDto`] the API returns, [`SandboxError`];
//! * [`ops`] — create, list and get;
//! * [`delete`] — the first half of a delete, under the sandbox's row lock;
//! * [`activity`] — when a sandbox was last used, and so when it expires;
//! * `view` — rows to the [`EnvironmentDto`], with the builds, owners and
//!   invocations they reference looked up once;
//! * [`handlers`] — the four routes under `/api/customer-apps/{id}/environments`;
//! * [`lock`] — one teardown or migration of a sandbox at a time;
//! * [`publish`] — what a publish that names a sandbox checks, moves and queues;
//! * [`migrations_task`] — a sandbox build's Airhouse migrations, queued;
//! * [`oltp_task`] — the sandbox's own schema on the org's OLTP staging
//!   branch, seeded and migrated, queued; [`oltp_home`] its steps and
//!   [`oltp_state`] what the row records of it;
//! * [`retention`] — which builds a publish may prune, sandbox builds apart;
//! * [`teardown`] — the second half of a delete, on the worker fleet;
//! * [`maintenance`] — the loop that expires idle sandboxes and retries
//!   teardowns that did not finish; [`token_ended`] its pass over the
//!   sandboxes of a sandbox agent token that ended.
//!
//! **Who.** Every route requires `Action::AppNonProduction` over the app
//! (`custom_apps_env_resolve::may_open_non_production` — the rule that opens
//! staging) on top of the staff console's own guards, and refuses a publish
//! token. Ownership is recorded and shown, not enforced.

pub mod activity;
pub mod agent_draft;
pub(crate) mod agent_publish;
pub mod delete;
mod expiry_audit;
pub mod handlers;
pub mod lock;
pub mod maintenance;
pub mod migrations_task;
pub mod oltp_home;
pub mod oltp_state;
pub mod oltp_task;
pub mod ops;
pub(crate) mod own;
pub(crate) mod publish;
pub(crate) mod retention;
pub mod teardown;
mod teardown_executor;
pub mod token_ended;
mod view;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// The most sandboxes one app may have. Rows still being torn down count, so
/// creating and deleting in a loop cannot outrun the teardowns.
pub const MAX_SANDBOXES_PER_APP: u64 = 20;

/// How many sandboxes one sandbox agent token may hold, across the apps it is
/// granted (sandbox agent credential design, decision 2), counting those still
/// being torn down. A crashed agent leaves at most this many behind.
pub const MAX_SANDBOXES_PER_TOKEN: u64 = 3;

pub const IDLE_DAYS_ENV: &str = "OXY_APP_SANDBOX_IDLE_DAYS";
const DEFAULT_IDLE_DAYS: i64 = 7;

/// How long a sandbox may sit unused — no publish and no function invocation
/// — before the maintenance loop deletes it: `OXY_APP_SANDBOX_IDLE_DAYS`,
/// default 7, at least 1.
pub fn idle_ttl() -> chrono::Duration {
    let days = std::env::var(IDLE_DAYS_ENV)
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(DEFAULT_IDLE_DAYS)
        .max(1);
    chrono::Duration::days(days)
}

/// Who created a sandbox. Shown, never enforced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerDto {
    pub user_id: Uuid,
    pub email: Option<String>,
}

/// One environment of an app, as the API returns it
/// (`internal-docs/custom-app-sandboxes.md` §5.1). Every field is always
/// present; one that does not apply is `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EnvironmentDto {
    pub name: String,
    /// `production` | `staging` | `dev`.
    pub kind: String,
    /// `active` | `deleting`.
    pub status: String,
    /// The publish's build id; `null` when the environment serves nothing.
    pub build_id: Option<String>,
    /// `app_builds.id` — what log lines carry as `build_id`.
    pub build_uuid: Option<Uuid>,
    /// The build's semantic pin, if any.
    pub semantic_revision_id: Option<Uuid>,
    /// `null` for production and staging.
    pub owner: Option<OwnerDto>,
    pub created_at: DateTime<Utc>,
    /// The last pointer move.
    pub updated_at: DateTime<Utc>,
    /// A sandbox's last publish or function invocation; `null` otherwise.
    pub last_activity_at: Option<DateTime<Utc>>,
    /// When an idle sandbox is deleted; `null` otherwise.
    pub expires_at: Option<DateTime<Utc>>,
    /// The environment's host; `null` when the deployment has no custom-apps
    /// zone or the host label would exceed 63 bytes.
    pub url: Option<String>,
    /// A sandbox's own schema on the org's OLTP staging branch, once a
    /// publish has queued it; `null` otherwise.
    pub oltp_schema: Option<oltp_state::OltpSchemaDto>,
}

/// Why a sandbox is being torn down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TeardownReason {
    /// Someone asked for it.
    Deleted,
    /// It sat idle past [`idle_ttl`].
    Expired,
    /// Its teardown did not finish, and the maintenance loop queued it again.
    Retried,
    /// The sandbox agent token that created it was revoked or expired more
    /// than [`token_ended::grace`] ago.
    TokenEnded,
}

impl TeardownReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Expired => "expired",
            Self::Retried => "retried",
            Self::TokenEnded => "token_ended",
        }
    }
}

/// Every way a sandbox management call is refused. Answers
/// `{"error":"<code>","message":"<text>"}`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SandboxError {
    #[error(
        "{0:?} is not a sandbox name: use dev-<handle>, where the handle is 1 to 12 characters \
         of a-z, 0-9 and single hyphens, and does not start or end with a hyphen"
    )]
    InvalidName(String),
    #[error("{0} is not a sandbox: only a dev-<handle> environment can be created or deleted")]
    NotASandbox(String),
    #[error(
        "a publish token cannot use an app's sandboxes: use a login token (`oxyc login`) or an \
         API key"
    )]
    PublishToken,
    #[error(
        "sandboxes are for Oxy staff who may open this app's non-production environments \
         (develop_apps over its org)"
    )]
    NotStaff,
    #[error("app not found")]
    AppNotFound,
    #[error("this app has no environment {0}")]
    NotFound(String),
    #[error("this app already has a sandbox named {0}")]
    Exists(String),
    #[error("{0} is still being deleted; its name is free once the teardown finishes")]
    Deleting(String),
    #[error(
        "this app already has {0} sandboxes, counting those still being deleted; delete one first"
    )]
    Limit(u64),
    /// A sandbox agent token at its own limit, across the apps it is granted.
    #[error(
        "this token already holds {0} sandboxes, counting those still being deleted; delete one \
         of its own and wait for its teardown before creating another"
    )]
    TokenLimit(u64),
    /// The sandbox's Airhouse sibling would carry the name of another app's
    /// own schema (a legacy slug holding `--`).
    #[error(
        "{name} cannot be a sandbox of this app: its Airhouse schema would be the app {app}'s own; \
         pick another handle"
    )]
    Reserved { name: String, app: String },
    #[error("internal error")]
    Internal(String),
}

impl SandboxError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidName(_) | Self::NotASandbox(_) => StatusCode::BAD_REQUEST,
            Self::PublishToken | Self::NotStaff => StatusCode::FORBIDDEN,
            Self::AppNotFound | Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Exists(_)
            | Self::Deleting(_)
            | Self::Limit(_)
            | Self::TokenLimit(_)
            | Self::Reserved { .. } => StatusCode::CONFLICT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// The machine-readable code beside the message.
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidName(_) => "invalid_environment_name",
            Self::NotASandbox(_) => "not_a_sandbox",
            Self::PublishToken => "publish_token_refused",
            Self::NotStaff => "non_production_refused",
            Self::AppNotFound => "app_not_found",
            Self::NotFound(_) => "environment_not_found",
            Self::Exists(_) => "environment_exists",
            Self::Deleting(_) => "environment_deleting",
            Self::Limit(_) => "environment_limit",
            Self::TokenLimit(_) => "token_sandbox_limit",
            Self::Reserved { .. } => "environment_reserved",
            Self::Internal(_) => "internal",
        }
    }

    /// A database failure: logged with its detail, answered without it.
    pub(crate) fn db(context: &str, error: impl std::fmt::Display) -> Self {
        tracing::error!("custom_apps_sandboxes: {context}: {error}");
        Self::Internal(format!("{context}: {error}"))
    }
}

impl IntoResponse for SandboxError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({ "error": self.code(), "message": self.to_string() });
        (self.status(), Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract's table (`internal-docs/custom-app-sandboxes.md` §5.1):
    /// one status and one code per refusal, and no two refusals share a code.
    #[test]
    fn every_refusal_has_its_status_and_code() {
        let table = [
            (
                SandboxError::InvalidName("x".into()),
                400,
                "invalid_environment_name",
            ),
            (
                SandboxError::NotASandbox("staging".into()),
                400,
                "not_a_sandbox",
            ),
            (SandboxError::PublishToken, 403, "publish_token_refused"),
            (SandboxError::NotStaff, 403, "non_production_refused"),
            (SandboxError::AppNotFound, 404, "app_not_found"),
            (
                SandboxError::NotFound("dev-a".into()),
                404,
                "environment_not_found",
            ),
            (
                SandboxError::Exists("dev-a".into()),
                409,
                "environment_exists",
            ),
            (
                SandboxError::Deleting("dev-a".into()),
                409,
                "environment_deleting",
            ),
            (SandboxError::Limit(20), 409, "environment_limit"),
            (SandboxError::TokenLimit(3), 409, "token_sandbox_limit"),
            (
                SandboxError::Reserved {
                    name: "dev-a".into(),
                    app: "x--dev-a".into(),
                },
                409,
                "environment_reserved",
            ),
            (SandboxError::Internal("boom".into()), 500, "internal"),
        ];
        let mut codes = std::collections::BTreeSet::new();
        for (error, status, code) in table {
            assert_eq!(error.status().as_u16(), status, "{error:?}");
            assert_eq!(error.code(), code, "{error:?}");
            assert!(codes.insert(code), "{code} names two refusals");
        }
        assert_eq!(codes.len(), 12);
    }

    #[tokio::test]
    async fn a_refusal_answers_error_and_message() {
        let response = SandboxError::Limit(20).into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .expect("body");
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(body["error"], "environment_limit");
        assert!(body["message"].as_str().unwrap().contains("20 sandboxes"));
        assert_eq!(body.as_object().unwrap().len(), 2, "error and message only");
    }

    /// An internal failure's detail is logged, never answered.
    #[test]
    fn an_internal_error_does_not_leak_its_detail() {
        let error = SandboxError::Internal("relation \"apps\" does not exist".into());
        assert_eq!(error.to_string(), "internal error");
    }

    /// One week by default; the override is whole days, never under one.
    /// nextest runs each test in its own process, so the env writes are its own.
    #[test]
    fn idle_ttl_is_a_week_unless_overridden_and_never_under_a_day() {
        // SAFETY: nextest runs each test in its own process.
        unsafe { std::env::remove_var(IDLE_DAYS_ENV) };
        assert_eq!(idle_ttl(), chrono::Duration::days(7));
        for (raw, days) in [("3", 3), (" 14 ", 14), ("0", 1), ("-5", 1), ("soon", 7)] {
            unsafe { std::env::set_var(IDLE_DAYS_ENV, raw) };
            assert_eq!(idle_ttl(), chrono::Duration::days(days), "{raw:?}");
        }
    }

    #[test]
    fn the_environment_object_always_carries_every_field() {
        let now = Utc::now();
        let dto = EnvironmentDto {
            name: "dev-a1".into(),
            kind: "dev".into(),
            status: "active".into(),
            build_id: None,
            build_uuid: None,
            semantic_revision_id: None,
            owner: None,
            created_at: now,
            updated_at: now,
            last_activity_at: None,
            expires_at: None,
            url: None,
            oltp_schema: None,
        };
        let json = serde_json::to_value(&dto).expect("json");
        let object = json.as_object().expect("an object");
        for field in [
            "name",
            "kind",
            "status",
            "build_id",
            "build_uuid",
            "semantic_revision_id",
            "owner",
            "created_at",
            "updated_at",
            "last_activity_at",
            "expires_at",
            "url",
            "oltp_schema",
        ] {
            assert!(object.contains_key(field), "{field} is missing");
        }
        assert_eq!(object.len(), 13);
        assert!(json["build_id"].is_null(), "null, not omitted");
    }
}
