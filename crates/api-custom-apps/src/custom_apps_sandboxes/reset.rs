//! `POST /api/customer-apps/{id}/environments/{name}/reset` — put planes of an
//! environment back to what its parent has
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.5).
//!
//! The body is `{"planes": ["storage", …]}`. The route is the seam: it
//! validates the request and walks the planes in the order given, handing each
//! to [`reset_plane`], whose one exhaustive `match` has an arm for every
//! [`Plane`]. A phase that ships a plane's reset replaces that plane's arm; a
//! plane added to [`Plane`] does not compile until it has one. `storage`,
//! `secrets`, `messages` and `airhouse` have one (`reset_arms`), and `oltp`
//! its own (`reset_oltp`); every other arm answers `409 plane_not_resettable`.
//! Storage and secrets reset on sandboxes only: staging's are shared by the
//! team, and staging's OLTP is the org's staging branch.
//!
//! **OLTP** (P2): a sandbox on its own branch re-cuts it from production
//! ([`super::reset_oltp`]), queued as a run.
//!
//! **Airhouse** (P3): the overlay's copied rows go and its tables are
//! forgotten, so reads fall back to production; under `snapshot` the pin is
//! set to now (`reset_arms::airhouse`).
//!
//! The walk holds the sandbox's lock (`SandboxLock`, the one a teardown and a
//! migration take) and reads its row under it: a sandbox another run holds is
//! `409 environment_busy`, and one being torn down is not found.
//!
//! A walk stops at the first refusal, so a plane reset before it stays reset;
//! a client that wants all-or-nothing resets one plane at a time.
//!
//! The same door as `PUT …/planes` (`Door::Sandbox`). Production is
//! `409 production_has_no_planes`; an empty list or an unknown key is
//! `400 invalid_planes`.

use std::time::Duration;

use axum::extract::Path;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use entity::apps;
use oxy_app_core::audit::{self, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::handlers::{Admitted, Door, admit, parse};
use super::lock::SandboxLock;
use super::ops::scoped;
use super::reset_arms;
use super::reset_oltp::reset_oltp;
use super::{SandboxError, branch};
use crate::custom_apps_env_resolve::sandbox_row;
use crate::custom_apps_planes::Plane;

/// The planes with a reset in this release.
pub(crate) const RESETTABLE: [Plane; 5] = [
    Plane::Oltp,
    Plane::Storage,
    Plane::Secrets,
    Plane::Messages,
    Plane::Airhouse,
];

#[derive(Debug, Deserialize)]
pub struct ResetRequest {
    /// Plane keys, as `env show` spells them.
    pub planes: Vec<String>,
    /// An RFC 3339 time to fork the reset planes from the parent as of;
    /// left out, from the parent as it is now. Only the OLTP branch takes
    /// one: given without `oltp` among the planes, it is `400 invalid_fork_at`.
    #[serde(default)]
    pub at: Option<String>,
}

/// How far one plane's reset has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetStatus {
    /// Finished before the request answered.
    Done,
    /// Queued as a run; `run_id` names it.
    Queued,
}

/// One plane's reset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaneReset {
    pub plane: &'static str,
    pub status: ResetStatus,
    /// The queued run, when `status` is `queued`; `null` when it is `done`.
    pub run_id: Option<String>,
}

/// `{"name", "planes": [{"plane", "status", "run_id"}]}`, one entry per plane
/// asked for, in the order asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResetAnswer {
    pub name: String,
    pub planes: Vec<PlaneReset>,
}

/// What a plane's reset is handed: the environment it puts back, and who
/// asked.
pub struct ResetTarget<'a> {
    pub db: &'a DatabaseConnection,
    pub app: &'a apps::Model,
    pub environment: &'a AppEnvironment,
    pub actor: &'a RequestActor,
    /// The request's `at`, parsed: not in the future.
    pub at: Option<DateTime<Utc>>,
}

/// `POST /api/customer-apps/{id}/environments/{name}/reset`
pub async fn reset(
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    Path((id, name)): Path<(Uuid, String)>,
    Json(body): Json<ResetRequest>,
) -> Result<Json<ResetAnswer>, SandboxError> {
    let Admitted { db, app, .. } = admit(&actor.user, marker, id, Door::Sandbox(&name)).await?;
    let environment = parse(&name)?;
    if environment == AppEnvironment::Production {
        return Err(SandboxError::ProductionHasNoPlanes);
    }
    let planes = requested_planes(&body.planes)?;
    let at = body
        .at
        .as_deref()
        .map(|at| branch::parse_fork_at(at, Utc::now()).map(|at| at.with_timezone(&Utc)))
        .transpose()?;
    refuse_at_without_oltp(at, &planes)?;
    let target = ResetTarget {
        db: &db,
        app: &app,
        environment: &environment,
        actor: &actor,
        at,
    };
    let done = walk(&target, planes).await?;
    Ok(Json(ResetAnswer {
        name: environment.name(),
        planes: done,
    }))
}

/// How long a reset waits for a sandbox another run holds, before it says so.
const LOCK_WAITS: [Duration; 2] = [Duration::from_millis(250), Duration::from_millis(750)];

/// Reset `planes` in order, under the environment's lock when any of them has
/// a reset: a plane with none is refused without waiting for anyone.
async fn walk(
    target: &ResetTarget<'_>,
    planes: Vec<Plane>,
) -> Result<Vec<PlaneReset>, SandboxError> {
    let name = target.environment.name();
    let lock = if planes.iter().any(|plane| RESETTABLE.contains(plane)) {
        let held = SandboxLock::acquire(target.db, target.app.id, &name, &LOCK_WAITS)
            .await
            .map_err(|e| SandboxError::db("take the environment's lock", e))?;
        Some(held.ok_or_else(|| SandboxError::Busy(name.clone()))?)
    } else {
        None
    };
    let outcome = walk_locked(target, planes).await;
    if let Some(lock) = lock {
        lock.release().await;
    }
    outcome
}

/// The walk itself. A sandbox is read again here, under the lock: one that
/// began to be torn down since the request was admitted is not reset, and a
/// sandbox agent token must still own it, since while it waited for the lock
/// a teardown may have freed the name and a colleague created another under it.
async fn walk_locked(
    target: &ResetTarget<'_>,
    planes: Vec<Plane>,
) -> Result<Vec<PlaneReset>, SandboxError> {
    if matches!(target.environment, AppEnvironment::Dev { .. }) {
        let row = sandbox_row(target.db, target.app.id, target.environment)
            .await
            .map_err(|e| SandboxError::db("read the sandbox", e))?
            .ok_or_else(|| SandboxError::NotFound(target.environment.name()))?;
        super::delete::refuse_unless_own(&row, Some(target.actor))?;
    }
    let mut done = Vec::with_capacity(planes.len());
    for plane in planes {
        let reset = reset_plane(target, plane).await?;
        record(target, &reset).await;
        done.push(reset);
    }
    Ok(done)
}

/// The audit row of one plane's reset: who emptied what, of which environment.
async fn record(target: &ResetTarget<'_>, reset: &PlaneReset) {
    let entry = audit::AuditEntry::for_request(target.actor, "app.environment.plane_reset")
        .metadata(serde_json::json!({ "plane": reset.plane }));
    audit::record_best_effort(target.db, scoped(entry, target.app, target.environment)).await;
}

/// `at` is a fork point, and only the OLTP branch forks: a reset naming one
/// without `oltp` among its planes is `400 invalid_fork_at`, never a request
/// whose `at` is silently dropped.
pub(super) fn refuse_at_without_oltp(
    at: Option<DateTime<Utc>>,
    planes: &[Plane],
) -> Result<(), SandboxError> {
    if at.is_some() && !planes.contains(&Plane::Oltp) {
        return Err(SandboxError::InvalidForkAt(
            "at is a fork point for the oltp plane only: add \"oltp\" to the planes, or leave \
             at out"
                .to_string(),
        ));
    }
    Ok(())
}

/// The planes `keys` name, in the order given and each once. An empty list or
/// an unknown key is `InvalidPlanes`.
fn requested_planes(keys: &[String]) -> Result<Vec<Plane>, SandboxError> {
    if keys.is_empty() {
        return Err(SandboxError::InvalidPlanes(
            "name at least one plane to reset, like {\"planes\": [\"storage\"]}".to_string(),
        ));
    }
    let mut planes = Vec::with_capacity(keys.len());
    for key in keys {
        let plane = Plane::ALL
            .iter()
            .copied()
            .find(|plane| plane.key() == key)
            .ok_or_else(|| {
                let known: Vec<&str> = Plane::ALL.iter().map(|plane| plane.key()).collect();
                SandboxError::InvalidPlanes(format!(
                    "{key:?} is not a plane: use one of {}",
                    known.join(", ")
                ))
            })?;
        if !planes.contains(&plane) {
            planes.push(plane);
        }
    }
    Ok(planes)
}

/// Put one plane of `target.environment` back. One arm per plane, no wildcard:
/// a plane added to [`Plane`] fails to compile here until it says what its
/// reset is. A phase that ships one replaces its arm and returns `Done`, or
/// `Queued` with the run's id.
async fn reset_plane(target: &ResetTarget<'_>, plane: Plane) -> Result<PlaneReset, SandboxError> {
    let not_resettable = || Err(SandboxError::PlaneNotResettable(plane.key().to_string()));
    match plane {
        Plane::Build => not_resettable(),
        Plane::Semantic => not_resettable(),
        Plane::Reads => not_resettable(),
        Plane::Oltp => reset_oltp(target).await,
        Plane::Airhouse => reset_arms::airhouse(target).await,
        Plane::Warehouse => not_resettable(),
        Plane::Storage => reset_arms::storage(target).await,
        Plane::Secrets => reset_arms::secrets(target).await,
        Plane::Messages => reset_arms::messages(target).await,
        Plane::Fetch => not_resettable(),
        Plane::Schedules => not_resettable(),
        Plane::WorkspaceActions => not_resettable(),
    }
}

#[cfg(test)]
#[path = "reset_tests.rs"]
mod tests;
