//! What each resettable plane's reset does (`reset::reset_plane` picks one):
//! the environment's own home of that plane, emptied, and nothing beside it.
//!
//! - **storage**: the environment's silo. A sandbox's alone; staging's files
//!   are shared by the team and are not reset from here.
//! - **secrets**: the environment's own secret path (the cache of it with
//!   it). A sandbox's alone, for the same reason; production's keys and
//!   staging's are never named.
//! - **messages**: `_outbox/` of the environment's silo. Staging's too: the
//!   outbox holds only what a function captured, and nobody else's files.
//! - **airhouse** (P3): the overlay's copied rows go and its tables are
//!   forgotten, so reads fall back to production; under `snapshot` the pin
//!   is set to now (`airhouse_overlay::reset_airhouse_overlay`). Staging's
//!   is always `write-only`, so it holds no copies to empty.
//!
//! The teardown's helpers do the deleting. They are Dev-only for the whole
//! silo and the whole secret path, and keep that guard: a staging that
//! reaches them is refused here first, and by them if it ever got through.

use oxy_app_core::custom_app_environment::AppEnvironment;

use super::SandboxError;
use super::airhouse_overlay::reset_airhouse_overlay;
use super::reset::{PlaneReset, ResetStatus, ResetTarget};
use crate::custom_apps_migrations::AirhouseDrop;
use crate::custom_apps_outbox;
use crate::custom_apps_planes::{AirhouseMode, Plane, environment_planes};
use crate::custom_apps_secrets::delete_environment_secrets;
use crate::custom_apps_storage::{delete_environment_assets, delete_environment_prefix};

fn done(plane: Plane) -> PlaneReset {
    PlaneReset {
        plane: plane.key(),
        status: ResetStatus::Done,
        run_id: None,
    }
}

/// A sandbox's plane to reset: staging's is the team's.
fn sandbox_only(target: &ResetTarget<'_>, plane: Plane) -> Result<(), SandboxError> {
    if matches!(target.environment, AppEnvironment::Dev { .. }) {
        return Ok(());
    }
    Err(SandboxError::PlaneNotResettableHere(format!(
        "the {} plane of {} cannot be reset from here: its {} are shared by the team, so only a \
         sandbox's are reset",
        plane.key(),
        target.environment,
        if plane == Plane::Storage {
            "files"
        } else {
            "secrets"
        },
    )))
}

/// Empty the environment's storage silo.
pub(super) async fn storage(target: &ResetTarget<'_>) -> Result<PlaneReset, SandboxError> {
    sandbox_only(target, Plane::Storage)?;
    delete_environment_assets(target.app.id, target.environment)
        .await
        .map_err(|e| SandboxError::db("delete the sandbox's storage", e))?;
    Ok(done(Plane::Storage))
}

/// Delete the environment's own secrets.
pub(super) async fn secrets(target: &ResetTarget<'_>) -> Result<PlaneReset, SandboxError> {
    sandbox_only(target, Plane::Secrets)?;
    let app = target.app;
    delete_environment_secrets(target.db, app.project_id, app.id, target.environment)
        .await
        .map_err(|e| SandboxError::db("delete the sandbox's secrets", e))?;
    Ok(done(Plane::Secrets))
}

/// Delete the environment's captured messages.
pub(super) async fn messages(target: &ResetTarget<'_>) -> Result<PlaneReset, SandboxError> {
    delete_environment_prefix(
        target.app.id,
        target.environment,
        custom_apps_outbox::PREFIX,
    )
    .await
    .map_err(|e| SandboxError::db("delete the outbox", e))?;
    Ok(done(Plane::Messages))
}

/// Empty what the overlay copied, forget it, and re-pin a `snapshot`
/// environment to now. Done before the request answers.
pub(super) async fn airhouse(target: &ResetTarget<'_>) -> Result<PlaneReset, SandboxError> {
    let planes = environment_planes(target.db, target.app.id, target.environment)
        .await
        .map_err(SandboxError::Internal)?
        .ok_or(SandboxError::ProductionHasNoPlanes)?;
    let run = AirhouseDrop {
        app_id: target.app.id,
        app_slug: &target.app.slug,
        workspace_id: target.app.project_id,
    };
    let snapshot = planes.airhouse == AirhouseMode::Snapshot;
    let outcome = reset_airhouse_overlay(target.db, run, target.environment, snapshot, None)
        .await
        .map_err(SandboxError::Internal)?;
    tracing::info!(
        app_id = %target.app.id,
        environment = %target.environment,
        emptied = outcome.tables_emptied.len(),
        pinned_at = ?outcome.pinned_at,
        "Airhouse overlay reset"
    );
    Ok(done(Plane::Airhouse))
}
