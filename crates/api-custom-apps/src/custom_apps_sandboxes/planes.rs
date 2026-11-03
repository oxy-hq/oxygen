//! `PUT /api/customer-apps/{id}/environments/{name}/planes` — choose the
//! modes of an environment's planes
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.5).
//!
//! **The body replaces the stored object.** A plane left out takes the default
//! for the environment's kind, so a client that changes one plane reads
//! `planes_set` from the environment, merges into it, and sends the whole
//! object back.
//!
//! The same door as deleting a sandbox (`Door::Sandbox`): a person who may
//! open the environment passes, and a sandbox agent token passes only for a
//! sandbox it created — staging and production are answered `404` to a token.
//! For a person: production is `409 production_has_no_planes` (checked first,
//! before any read), staging with no `app_environments` row is
//! `409 environment_has_no_row` (a row inserted here would carry no build and
//! stop staging serving the build its legacy column names), and a choice that
//! does not parse or validate is `400 invalid_planes`.
//!
//! **The `semantic` plane has a side effect beyond the row**
//! ([`super::planes_pin`]): a `branch:<name>` choice asks the workspace for
//! the branch before anything is stored (`400 invalid_planes` naming it when
//! there is none), and once stored the environment is pinned to the branch's
//! compiled revision; any other choice clears a pin the environment had, and
//! so does a different branch (in the store's own transaction, before the new
//! pin is set). A failed pin is answered with its own status after the store,
//! and sending the same request again re-pins. Until it does, the reader
//! refuses the environment (`503 semantic_branch_not_ready`, naming the
//! branch): a row on a branch with no pin is never served another model.

use std::sync::Arc;

use std::collections::BTreeMap;

use axum::extract::Path;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use entity::{app_builds, app_environments, apps};
use oxy_app_core::audit::{self, RequestActor};
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::semantic_pin_seam::SemanticPinner;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
    QuerySelect, TransactionTrait,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::delete::refuse_unless_own;
use super::handlers::{Admitted, Door, admit, parse};
use super::ops::{get_for, scoped};
use super::planes_pin::{self, Pinner};
use super::{EnvironmentDto, SandboxError};
use crate::custom_apps_env_branch::planes as branch_planes;
use crate::custom_apps_env_resolve::resolve_function_environment;
use crate::custom_apps_functions::env_policy::EnvPolicy;
use crate::custom_apps_nonproduction::{check_overrides, destinations_from_build_manifest};
use crate::custom_apps_planes::{PlaneContext, PlaneSpec, RequestedPlanes};

/// The audit action of a change of planes.
const ACTION: &str = "app.environment.planes_set";

#[derive(Debug, Deserialize)]
pub struct SetPlanesRequest {
    /// Plane key → mode. Replaces the stored object.
    pub planes: Value,
}

/// `PUT /api/customer-apps/{id}/environments/{name}/planes` — `200` with the
/// environment as it is now.
pub async fn set(
    actor: RequestActor,
    marker: Option<Extension<AppPublishTokenAuth>>,
    pinner: Option<Extension<Arc<dyn SemanticPinner>>>,
    Path((id, name)): Path<(Uuid, String)>,
    Json(body): Json<SetPlanesRequest>,
) -> Result<Json<EnvironmentDto>, SandboxError> {
    let Admitted {
        db,
        app,
        org_slug,
        own,
    } = admit(&actor.user, marker, id, Door::Sandbox(&name)).await?;
    let environment = parse(&name)?;
    if environment == AppEnvironment::Production {
        return Err(SandboxError::ProductionHasNoPlanes);
    }
    let pinner = pinner.as_ref().map(|Extension(pinner)| pinner);
    let requested = checked(&db, &app, &environment, &body.planes, pinner).await?;
    let branch = requested.spec.semantic_branch();
    replace(&db, &app, &environment, &actor, requested.stored, branch).await?;
    planes_pin::settle(
        pinner,
        &app,
        &environment,
        branch,
        planes_pin::PinAfter::PlanesWrite,
    )
    .await?;
    Ok(Json(
        get_for(&db, &app, &org_slug, &environment, own).await?,
    ))
}

/// `planes` as the request to store: an object that parses strictly and
/// passes `validate` for `environment`, its `semantic` branch (if any) found.
async fn checked(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    planes: &Value,
    pinner: Pinner<'_>,
) -> Result<RequestedPlanes, SandboxError> {
    if !planes.is_object() {
        return Err(SandboxError::InvalidPlanes(
            "planes must be an object of plane key to mode, like {\"storage\": \"branch\"}"
                .to_string(),
        ));
    }
    let serving = resolve_function_environment(db, app, environment)
        .await
        .map_err(|e| SandboxError::db("resolve the environment's build", e))?;
    let ask = (None, Some(planes));
    planes_pin::requested(db, app, environment, ask, (serving.build_id, None), pinner).await
}

/// Whether `spec` may be stored for `environment`: the facts its modes depend
/// on, loaded (`PlaneContext::load`, plus the workspace check of the warehouse
/// overrides), then `PlaneSpec::validate`. A refusal is `400 invalid_planes`.
/// The one door both writers of `planes` (this route and the create) use,
/// through [`planes_pin::requested`].
///
/// `build` is the build the environment will serve once the choice is
/// stored: its own for this route, and for a create the build copied from
/// `from` (`copy_build::forked_build`; none without `from`), because the row
/// does not exist yet to be resolved. The overrides are checked against that
/// build's mapping.
///
/// A create passes its `at` as `fork_at`, which no row holds yet either, and
/// `existing_branch` is the `semantic` branch the workspace was asked for and
/// has.
pub(super) async fn validate_choice(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    spec: &PlaneSpec,
    build: Option<Uuid>,
    (fork_at, existing_branch): (Option<DateTime<Utc>>, Option<&str>),
) -> Result<(), SandboxError> {
    let mut ctx = PlaneContext::load(db, app, environment)
        .await
        .map_err(|e| SandboxError::db("load the plane context", e))?
        .with_fork_at(fork_at);
    if let Some(branch) = existing_branch {
        ctx = ctx.with_existing_branch(branch);
    }
    if let Some(warehouse) = spec
        .warehouse
        .as_ref()
        .filter(|w| !w.destinations.is_empty())
    {
        let merged = merged_destinations(db, app, environment, spec, build).await?;
        ctx.warehouse_refusal =
            check_overrides(app.project_id, &merged, &warehouse.destinations).await;
    }
    spec.validate(&ctx)
        .map_err(|e| SandboxError::InvalidPlanes(e.to_string()))
}

/// The mapping `environment` would run with under `spec`: the `build` it
/// serves (`nonProduction.destinations`, as `run_policy` reads it), under the
/// warehouse overrides, key by key. Built by the policy itself
/// (`EnvPolicy::with_destinations`), so the check here and the host's
/// per-write re-check see one map. An environment with no build has an empty
/// mapping of its own.
async fn merged_destinations(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    spec: &PlaneSpec,
    build: Option<Uuid>,
) -> Result<BTreeMap<String, String>, SandboxError> {
    let manifest = match build {
        Some(build) => app_builds::Entity::find()
            .select_only()
            .column(app_builds::Column::ManifestJson)
            .filter(app_builds::Column::Id.eq(build))
            .into_tuple::<Option<Value>>()
            .one(db)
            .await
            .map_err(|e| SandboxError::db("read the build's manifest", e))?
            .flatten(),
        None => None,
    };
    let policy = EnvPolicy::for_environment(environment.clone());
    let policy = match spec.resolve(environment) {
        Some(planes) => policy.with_planes(planes),
        None => policy,
    };
    Ok(policy
        .with_destinations(destinations_from_build_manifest(manifest.as_ref(), app.id))
        .destinations()
        .clone())
}

/// Under a lock on the environment's row (the one a delete takes): store
/// `stored` as its `planes`, then audit the change. The row must exist and not
/// be tearing down.
///
/// **A write that changes what the `semantic` plane reads drops the old pin in
/// the same transaction**: when `branch` (the new choice's) is not the one the
/// row named, or is none, both pin columns are cleared with the store. The pin
/// that [`planes_pin::settle`] then sets belongs to the new branch, and if it
/// fails the row is a branch with no pin (refused by the reader), never the
/// previous branch's revision under a row that names another. The same branch
/// again keeps its pin, which `settle` refreshes.
async fn replace(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    actor: &RequestActor,
    stored: Value,
    branch: Option<&str>,
) -> Result<(), SandboxError> {
    let name = environment.name();
    let txn = db.begin().await.map_err(|e| SandboxError::db("begin", e))?;
    let row = app_environments::Entity::find_by_id((app.id, name.clone()))
        .lock_exclusive()
        .one(&txn)
        .await
        .map_err(|e| SandboxError::db("lock the environment", e))?;
    let row = match row {
        Some(row) => row,
        None if *environment == AppEnvironment::Staging => {
            return Err(SandboxError::EnvironmentHasNoRow(name));
        }
        None => return Err(SandboxError::NotFound(name)),
    };
    refuse_unless_own(&row, Some(actor))?;
    if row.deleting_at.is_some() {
        return Err(SandboxError::Deleting(name));
    }
    let before = row.planes.clone();
    let previous = PlaneSpec::parse(&before)
        .ok()
        .and_then(|spec| spec.semantic_branch().map(str::to_string));
    let mut changed: app_environments::ActiveModel = row.into();
    changed.planes = ActiveValue::Set(stored.clone());
    if branch.is_none() || branch != previous.as_deref() {
        changed.semantic_revision_id = ActiveValue::Set(None);
        changed.semantic_git_sha = ActiveValue::Set(None);
    }
    let updated = changed
        .update(&txn)
        .await
        .map_err(|e| SandboxError::db("store the planes", e))?;
    // Queued with the write: a sandbox whose planes ask for a branch is never
    // stored without the cut that makes one (or the drop that removes it).
    branch_planes::queue_for_change(&txn, app, &updated, &before)
        .await
        .map_err(|e| SandboxError::db("queue the OLTP branch task", e))?;
    txn.commit()
        .await
        .map_err(|e| SandboxError::db("commit", e))?;
    let entry = audit::AuditEntry::for_request(actor, ACTION)
        .change(json!({ "planes": before }), json!({ "planes": stored }));
    audit::record_best_effort(db, scoped(entry, app, environment)).await;
    Ok(())
}
