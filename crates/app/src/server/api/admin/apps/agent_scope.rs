//! A **sandbox agent token** (`oxy_sbx_`) on the verify and read-back routes
//! (sandbox agent credential design §1, rows C1–C3 and R1–R3).
//!
//! Every other caller of these routes may name production, or name nothing
//! and read it. The token may not: on each route the environment is
//! **required**, and must be a sandbox the token created, of the app in the
//! path. Anything else is answered `404`, as a missing environment is, so the
//! token learns nothing about production, staging or another creator's
//! sandbox.
//!
//! One decision, [`require_own`], asked by `environment_scope` wherever a
//! request names an environment and by the handlers that find one on a stored
//! row (a run, an invocation). It goes through oxy-authz
//! (`may_open_environment`), which binds the app as well as the sandbox.

use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use entity::apps;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::environment_scope::ScopeError;
use crate::server::api::custom_apps_env_resolve::may_open_environment;
use crate::server::api::custom_apps_sandbox_instance::own_since;

/// The sandbox agent token `user` authenticated with, if that is their
/// credential.
pub(crate) fn token_of(user: &AuthenticatedUser) -> Option<Uuid> {
    user.credential
        .as_ref()
        .filter(|credential| credential.is_sandbox_agent())
        .map(|credential| credential.token_id)
}

pub(crate) fn is_agent(user: &AuthenticatedUser) -> bool {
    token_of(user).is_some()
}

/// What the token is told wherever it may not act: the environment does not
/// exist. The code the sandbox routes use for a missing one.
pub(crate) fn not_found() -> ScopeError {
    ScopeError::new(
        StatusCode::NOT_FOUND,
        "environment_not_found",
        "this app has no such environment",
    )
}

/// Hold a sandbox agent token to `environment` being a sandbox it created,
/// of `app`. Call only for such a token: [`is_agent`].
pub(crate) async fn require_own(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &AppEnvironment,
) -> Result<(), ScopeError> {
    let caller = crate::server::authz::Caller::from_user(user);
    if may_open_environment(db, &caller, app, environment).await {
        Ok(())
    } else {
        Err(not_found())
    }
}

/// [`require_own`] for an environment read off a stored row. A name this
/// build cannot parse names no sandbox.
pub(crate) async fn require_own_named(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &str,
) -> Result<(), ScopeError> {
    match AppEnvironment::parse(environment) {
        Some(environment) => require_own(db, app, user, &environment).await,
        None => Err(not_found()),
    }
}

/// Where the token's own sandbox `environment` of `app` starts, for a read of
/// rows kept under that name (`custom_apps_sandbox_instance`): the token sees
/// what was written from then on, and nothing of an earlier sandbox that had
/// the name. `None` for every other caller, who reads by name with no extra
/// read. A name that is not the token's own sandbox is a not-found.
pub(crate) async fn instance_since(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &AppEnvironment,
) -> Result<Option<DateTime<Utc>>, ScopeError> {
    let Some(token) = token_of(user) else {
        return Ok(None);
    };
    own_since(db, app.id, environment, token)
        .await
        .map_err(|e| ScopeError::internal("sandbox lookup failed", e))?
        .map(Some)
        .ok_or_else(not_found)
}

/// [`instance_since`] for an environment read off a stored row.
pub(crate) async fn instance_since_named(
    db: &DatabaseConnection,
    app: &apps::Model,
    user: &AuthenticatedUser,
    environment: &str,
) -> Result<Option<DateTime<Utc>>, ScopeError> {
    match AppEnvironment::parse(environment) {
        Some(environment) => instance_since(db, app, user, &environment).await,
        None if is_agent(user) => Err(not_found()),
        None => Ok(None),
    }
}

/// One audit row for a queued check run: the person as the actor, their key
/// or token stamped by `for_request`, the environment as the target. Written
/// for every credential, not a sandbox agent token alone.
pub(crate) async fn audit_run_queued(
    db: &DatabaseConnection,
    actor: &RequestActor,
    app: &apps::Model,
    environment: &AppEnvironment,
    (function, run_id): (&str, &str),
) {
    let entry = AuditEntry::for_request(actor, "app.function.run_queued")
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_function",
            format!("{}/{function}", app.id),
            format!("{}/{function}", app.slug),
        )
        .environment(environment.name())
        .metadata(serde_json::json!({ "run_id": run_id, "function": function }));
    audit::record_best_effort(db, entry).await;
}
