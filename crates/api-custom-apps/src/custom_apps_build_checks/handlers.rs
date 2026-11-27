//! The console's routes, mounted in the staff console nest (`admin::apps`,
//! `/api/customer-apps/{id}/…`) behind its four layers: owner-or-app-admin,
//! `Action::PlatformApps`, the grant's app scope and no active assume session.
//! FleetOk: Postgres, the build store and a queued task, no working copy.
//!
//! The read writes in one case: an attempt whose runner died after its last
//! check that counts is closed by the read that finds it so
//! (`super::settled::close_abandoned`), because nothing else will.
//!
//! Here, before anything is read: a sandbox agent token is refused
//! ([`RefuseSandboxAgent`]). Then the rows of a staging build are staging's
//! evidence — a check function's error, a page error — so, like a run queued
//! outside production (`admin::apps::function_run`), they are read only by a
//! caller with non-production reach (`Action::AppNonProduction`) and never by
//! a publish token: everyone else is answered the bare not-found an unknown
//! build gets ([`super::admit::reader`]). The platform checker is not one of
//! those callers, and the nest's layers refuse it before this runs: its
//! routes are [`super::runner`]'s.
//!
//! - `GET /api/customer-apps/{id}/builds/{build}/checks[?attempt=N][&environment=staging|production]`
//!   (staging when no environment is named; `production` reads the attempts
//!   run on the build after production moved to it).
//! - `POST /api/customer-apps/{id}/builds/{build}/checks/rerun`, audited as
//!   `app.build_checks_rerun`. Staging's attempts only.
//! - `GET /api/customer-apps/{id}/builds/{build}/checks/files/{name}`: one
//!   file an attempt kept (`super::files::get`).
//! - `POST /api/customer-apps/{id}/builds/{build}/checks/changes/seen`: a
//!   person marks the changed screens seen (`super::seen::mark`).
//!
//! `{build}` is the build's row id or its `build_id`.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use oxy::database::client::establish_connection;
use oxy_app_core::agent_refusal::RefuseSandboxAgent;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::types::AppPublishTokenAuth;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::admit;
use super::dto::{self, ChecksResponse};
use super::environment::CheckEnvironment;
pub use super::error::ChecksError;
use super::error::internal;
use super::queue;
use super::settled;
use super::store;
use crate::admin::apps::environment_scope::Caller;

#[derive(Debug, Default, Deserialize)]
pub struct ShowQuery {
    pub attempt: Option<i32>,
    /// `staging` or `production`; staging when absent.
    pub environment: Option<String>,
}

/// `GET /api/customer-apps/{id}/builds/{build}/checks`.
pub async fn show(
    _: RefuseSandboxAgent,
    actor: RequestActor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path((id, build)): Path<(Uuid, String)>,
    Query(q): Query<ShowQuery>,
) -> Result<Json<ChecksResponse>, ChecksError> {
    let db = establish_connection().await.map_err(internal)?;
    let caller = Caller::new(&actor.user, marker.as_ref());
    let (app, build) = admit::reader(&db, &caller, id, &build).await?;
    let environment = CheckEnvironment::named(q.environment.as_deref())?.name();
    let mut rows = store::rows_of_build(&db, build.id, environment).await?;
    if let Some(n) = q.attempt
        && !rows.iter().any(|r| r.attempt == n)
    {
        return Err(ChecksError::NotFound);
    }
    let built = (&app, &build);
    if settled::close_abandoned(&db, built, environment, &rows, q.attempt).await? {
        rows = store::rows_of_build(&db, build.id, environment).await?;
    }
    let now = chrono::Utc::now();
    Ok(Json(dto::response(
        &build,
        environment,
        rows,
        q.attempt,
        now,
    )))
}

#[derive(Debug, Serialize)]
pub struct RerunResponse {
    pub attempt: i32,
    pub run_id: String,
    pub state: &'static str,
}

/// `POST /api/customer-apps/{id}/builds/{build}/checks/rerun`.
pub async fn rerun(
    _: RefuseSandboxAgent,
    actor: RequestActor,
    marker: Option<axum::Extension<AppPublishTokenAuth>>,
    Path((id, build)): Path<(Uuid, String)>,
) -> Result<(StatusCode, Json<RerunResponse>), ChecksError> {
    let db = establish_connection().await.map_err(internal)?;
    let caller = Caller::new(&actor.user, marker.as_ref());
    let (app, build) = admit::reader(&db, &caller, id, &build).await?;
    let task = queue::rerun(&db, &app, &build)
        .await
        .map_err(ChecksError::Refused)?;
    let entry = AuditEntry::for_request(&actor, "app.build_checks_rerun")
        .org(app.org_id)
        .workspace(app.project_id)
        .target("custom_app", app.id.to_string(), app.slug.clone())
        .metadata(json!({
            "build": build.build_id,
            "build_pk": build.id,
            "environment": task.environment,
            "attempt": task.attempt,
        }));
    audit::record_best_effort(&db, entry).await;
    let response = RerunResponse {
        attempt: task.attempt,
        run_id: task.run_id(),
        state: "running",
    };
    Ok((StatusCode::ACCEPTED, Json(response)))
}
