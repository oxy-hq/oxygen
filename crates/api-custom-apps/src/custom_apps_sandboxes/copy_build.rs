//! A create with `from` starts the sandbox on its parent's build
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.5).
//!
//! Two steps, in the order a sandbox publish takes them. Under the create's
//! lock, [`parent_build`] reads the build the parent serves, and the create
//! points the new row at it (`record_move`, the same `publish` event a publish
//! writes). After the create commits, [`queue_migrations`] queues that build's
//! migrations into the sandbox's own homes, through the task a sandbox publish
//! queues (`publish::queue_migrations`) — never staging's, and never the
//! create's failure: a build whose migrations cannot be read or queued leaves
//! the sandbox created and the reason in the log, as a publish leaves a
//! warning.
//!
//! **Only the pointer is copied.** The sandbox's storage, secrets, OLTP schema
//! and Airhouse sibling are its own, so nothing the parent holds is shared. And
//! copying a build is not publishing it: the build keeps the mark its own
//! publisher gave it (`app_builds.published_token_id`), so a sandbox agent
//! token that forks its own sandbox marks nothing.

use entity::{app_builds, app_environments, apps};
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

use super::SandboxError;
use super::publish::{SandboxBuild, queue_migrations};
use crate::custom_apps_env_resolve::load_environment_builds;
use crate::custom_apps_migrations::declared_in_build;

/// The build `parent` serves: production's or staging's pointer as resolution
/// reads it (the row, or the legacy column where a row is missing), or a
/// sandbox's own among `sandboxes`. `None` when the parent serves nothing — a
/// create from it gets no build, as a create always did.
pub(super) async fn parent_build<C: ConnectionTrait>(
    txn: &C,
    app: &apps::Model,
    parent: &str,
    sandboxes: &[app_environments::Model],
) -> Result<Option<Uuid>, SandboxError> {
    match AppEnvironment::parse(parent) {
        Some(AppEnvironment::Production) => Ok(fixed(txn, app).await?.0),
        Some(AppEnvironment::Staging) => Ok(fixed(txn, app).await?.1),
        Some(AppEnvironment::Dev { .. }) => Ok(sandboxes
            .iter()
            .find(|row| row.name == parent)
            .and_then(|row| row.build_id)),
        None => Ok(None),
    }
}

/// [`parent_build`] before the create's transaction, for the planes check: the
/// build a sandbox cut from `parent` is about to serve, read from `db` with
/// the app's sandboxes (a sandbox parent's pointer is on its row). Not under
/// the create's lock, so a parent that moves in between is read as it was; the
/// host repeats the check on every write.
pub(super) async fn forked_build(
    db: &DatabaseConnection,
    app: &apps::Model,
    parent: &str,
) -> Result<Option<Uuid>, SandboxError> {
    let sandboxes = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app.id))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .all(db)
        .await
        .map_err(|e| SandboxError::db("read the app's sandboxes", e))?;
    parent_build(db, app, parent, &sandboxes).await
}

/// `(production's build, staging's build)`.
async fn fixed<C: ConnectionTrait>(
    txn: &C,
    app: &apps::Model,
) -> Result<(Option<Uuid>, Option<Uuid>), SandboxError> {
    let builds = load_environment_builds(txn, app)
        .await
        .map_err(|e| SandboxError::db("read the parent's build", e))?;
    Ok((builds.production, builds.staging))
}

/// Queue `build_pk`'s migrations for the sandbox `environment` into its own
/// homes, as a publish of it would: for a new sandbox started on its parent's
/// build, and for one whose OLTP plane moved back to `schema`, whose schema no
/// publish migrated while it ran on a branch of its own. `cause` says which,
/// in the log, where what a publish would return as warnings goes.
pub(super) async fn queue_served_migrations(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    build_pk: Uuid,
    cause: &'static str,
) {
    let build = match app_builds::Entity::find_by_id(build_pk).one(db).await {
        Ok(Some(build)) => build,
        Ok(None) => return,
        Err(e) => return not_queued(app, environment, cause, &e.to_string()),
    };
    let declared = match declared_in_build(&build, &app.slug).await {
        Ok(declared) => declared,
        Err(e) => return not_queued(app, environment, cause, &e.to_string()),
    };
    let sandbox = SandboxBuild {
        app_id: app.id,
        app_slug: &app.slug,
        workspace_id: app.project_id,
        org_id: app.org_id,
        build_pk,
        environment,
    };
    for warning in queue_migrations(db, sandbox, &declared.oltp, &declared.airhouse).await {
        tracing::warn!(app_id = %app.id, %environment, "{cause}: {warning}");
    }
}

fn not_queued(app: &apps::Model, environment: &AppEnvironment, cause: &str, why: &str) {
    tracing::warn!(app_id = %app.id, %environment, error = %why,
        "{cause}: the build's migrations were not queued; publish to the sandbox to queue them");
}
