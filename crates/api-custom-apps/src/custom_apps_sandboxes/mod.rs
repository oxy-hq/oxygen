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
//! * [`ops`] — create, list and get; [`branch`] the facts a create may record
//!   (`from`, `at`, `planes`);
//! * [`oltp_branch_dto`] — a sandbox's own OLTP branch, as `env show` reports it;
//! * [`env_diff`] — `GET …/diff`: a sandbox's OLTP branch against production;
//! * [`planes`] and [`reset`] — `PUT …/planes` and `POST …/reset`: choose the
//!   modes of an environment's planes, and put a plane back;
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
//! * [`maintenance`] — the loop that expires idle sandboxes (and those 30 days
//!   past a fork that copied production rows) and retries teardowns that did
//!   not finish; [`token_ended`] its pass over the
//!   sandboxes of a sandbox agent token that ended.
//!
//! **Who.** Every route requires `Action::AppNonProduction` over the app
//! (`custom_apps_env_resolve::may_open_non_production` — the rule that opens
//! staging) on top of the staff console's own guards, and refuses a publish
//! token. Ownership is recorded and shown, not enforced.

pub mod activity;
pub mod agent_draft;
pub(crate) mod agent_publish;
pub mod airhouse_overlay;
pub mod branch;
mod copies;
mod copy_build;
pub mod delete;
pub mod env_diff;
mod expiry_audit;
pub mod handlers;
pub mod lock;
pub mod maintenance;
pub mod migrations_task;
pub mod oltp_branch_dto;
pub mod oltp_home;
pub mod oltp_state;
pub mod oltp_task;
pub mod ops;
mod orphan_branches;
pub mod outbox;
pub(crate) mod own;
pub mod planes;
pub(crate) mod planes_pin;
pub(crate) mod publish;
pub mod reset;
mod reset_arms;
mod reset_oltp;
pub(crate) mod retention;
pub mod teardown;
mod teardown_branch;
mod teardown_executor;
pub mod token_ended;
mod unwanted_branches;
mod view;

use std::collections::BTreeMap;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::custom_apps_functions::env_policy::EnvBranchUnready;

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
    /// A sandbox's own OLTP branch (`planes.oltp` `branch` or `empty`), from
    /// the moment its cut is recorded; `null` for a sandbox on `schema`, for
    /// production and for staging.
    pub oltp_branch: Option<oltp_branch_dto::OltpBranchDto>,
    /// When a sandbox is deleted for holding a copy of production: 30 days
    /// (`EXPIRY_DAYS`) past `fork_at` while its planes copy production or its
    /// Airhouse overlay still lists a table it copied — the maintenance sweep's
    /// own rule (`copies`). Activity does not move it. `null` while it holds
    /// none, and for production and staging.
    pub copies_expire_at: Option<DateTime<Utc>>,
    /// The environment this one was cut from. Always `null` for production;
    /// otherwise its row's `parent_name`, else `production` for staging and
    /// `staging` for a sandbox.
    pub parent: Option<String>,
    /// The point on the parent it was cut at: its row's `fork_at`, else when
    /// the row was created. `null` for production and for a fixed environment
    /// with no row.
    pub fork_at: Option<DateTime<Utc>>,
    /// Plane key → mode, over the defaults for the kind. `null` for
    /// production, and when `planes_error` is set.
    pub planes: Option<BTreeMap<String, String>>,
    /// Why the row's `planes` could not be read (an unknown plane or mode);
    /// `null` when it could, and for production.
    pub planes_error: Option<String>,
    /// The row's own `planes` object, as stored: the choices made, not the
    /// modes they resolve to (`{}` when nothing is set). `null` for production
    /// and for a fixed environment with no row. A client that changes one
    /// plane reads this, merges into it and `PUT`s the whole object back.
    pub planes_set: Option<serde_json::Value>,
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
    /// It is 30 days past its fork point and a plane of it copies production
    /// rows (`maintenance::expires_from_fork`).
    BranchExpired,
}

impl TeardownReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deleted => "deleted",
            Self::Expired => "expired",
            Self::Retried => "retried",
            Self::TokenEnded => "token_ended",
            Self::BranchExpired => "oltp_branch_expired",
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
    /// `from` names no environment this one may be cut from.
    #[error(
        "{0:?} is not an environment to fork from: name production, staging, or an active \
         sandbox of this app"
    )]
    InvalidParent(String),
    /// `at` is not an RFC 3339 time, or is in the future.
    #[error("{0}")]
    InvalidForkAt(String),
    /// A `planes` choice, a `preset` or a `planes` list that cannot be used.
    #[error("{0}")]
    InvalidPlanes(String),
    #[error("production has no planes: only staging and sandboxes choose their planes' modes")]
    ProductionHasNoPlanes,
    /// A fixed environment with no `app_environments` row. Inserting one with
    /// no build would stop it serving the build its legacy column names.
    #[error(
        "{0} has no environment row yet, so it runs every plane in its default mode and has \
         nothing to change"
    )]
    EnvironmentHasNoRow(String),
    /// A plane with no reset in this release, or in the mode it runs in.
    #[error(
        "the {0} plane cannot be reset: it has no reset in this release, or none in the mode it \
         runs in"
    )]
    PlaneNotResettable(String),
    /// The semantic pin port refused (or is not installed): its own status and
    /// code, so a `503` stays a `503` and GitHub being down is not a `500`.
    #[error("{}", .0.message)]
    Pin(oxy_app_core::semantic_pin_seam::PinRefusal),
    /// `env diff` of a sandbox whose OLTP branch cannot be read yet.
    #[error("{}", .0.fix())]
    OltpBranchUnready(EnvBranchUnready),
    /// A plane that resets elsewhere but not on this environment; the message
    /// says why.
    #[error("{0}")]
    PlaneNotResettableHere(String),
    /// Another teardown, migration or reset of the sandbox holds its lock.
    #[error("another teardown, migration or reset of {0} is still running; try again shortly")]
    Busy(String),
    #[error("internal error")]
    Internal(String),
}

impl SandboxError {
    pub fn status(&self) -> StatusCode {
        match self {
            Self::InvalidName(_)
            | Self::NotASandbox(_)
            | Self::InvalidParent(_)
            | Self::InvalidForkAt(_)
            | Self::InvalidPlanes(_) => StatusCode::BAD_REQUEST,
            Self::PublishToken | Self::NotStaff => StatusCode::FORBIDDEN,
            Self::AppNotFound | Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Exists(_)
            | Self::Deleting(_)
            | Self::Limit(_)
            | Self::TokenLimit(_)
            | Self::Reserved { .. }
            | Self::ProductionHasNoPlanes
            | Self::EnvironmentHasNoRow(_)
            | Self::PlaneNotResettable(_)
            | Self::PlaneNotResettableHere(_)
            | Self::OltpBranchUnready(_)
            | Self::Busy(_) => StatusCode::CONFLICT,
            Self::Pin(refusal) => refusal.status,
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
            Self::InvalidParent(_) => "invalid_parent",
            Self::InvalidForkAt(_) => "invalid_fork_at",
            Self::InvalidPlanes(_) => "invalid_planes",
            Self::ProductionHasNoPlanes => "production_has_no_planes",
            Self::EnvironmentHasNoRow(_) => "environment_has_no_row",
            Self::PlaneNotResettable(_) | Self::PlaneNotResettableHere(_) => "plane_not_resettable",
            Self::OltpBranchUnready(_) => "oltp_branch_unready",
            Self::Busy(_) => "environment_busy",
            Self::Pin(refusal) => refusal.code,
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
#[path = "mod_tests.rs"]
mod tests;
