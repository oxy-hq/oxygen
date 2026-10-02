//! Create, list and show a sandbox. The first half of deleting one is
//! [`super::delete`]'s, re-exported here.
//!
//! Each write is one transaction. **Create** locks the app row, so two
//! concurrent creates see each other's row and the limit holds. **Delete**
//! marks the row, clears its pointer and queues the teardown together, so a
//! sandbox is never left serving while "deleted", nor marked with nothing
//! queued to finish the job — and never queues a second teardown while one
//! is still on its way.

use chrono::Utc;
use entity::{app_environments, apps};
use oxy_app_core::audit;
use oxy_app_core::custom_app_environment::{AppEnvironment, AppEnvironmentKind};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr,
    EntityTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait,
};
use uuid::Uuid;

pub use super::delete::{Deletion, Expect, begin_delete, delete, delete_if};
use super::view::{View, is_sandbox};
use super::{EnvironmentDto, MAX_SANDBOXES_PER_APP, SandboxError};
use crate::server::api::custom_apps_env_resolve::load_environment_builds;
use crate::server::api::custom_apps_migrations::{AirhouseHome, schema_owner};

/// The audit actor of an expiry: no human asked for it.
const EXPIRY_ACTOR: &str = "system:sandbox-expiry";

/// `environment`, when it is a sandbox; `NotASandbox` for production and
/// staging, which are never created or deleted.
pub(super) fn require_sandbox(environment: &AppEnvironment) -> Result<(), SandboxError> {
    match environment {
        AppEnvironment::Dev { .. } => Ok(()),
        other => Err(SandboxError::NotASandbox(other.name())),
    }
}

/// Create the sandbox `environment` of `app`, owned by `owner`, with **no
/// build**. Refused when the name is taken (`Exists`), still being torn down
/// (`Deleting`), the app already has [`MAX_SANDBOXES_PER_APP`] — rows being
/// torn down count — or the sandbox's Airhouse sibling would carry the name
/// of another app's own schema (`Reserved`).
pub async fn create(
    db: &DatabaseConnection,
    app: &apps::Model,
    environment: &AppEnvironment,
    owner: Uuid,
) -> Result<app_environments::Model, SandboxError> {
    require_sandbox(environment)?;
    let txn = db.begin().await.map_err(|e| SandboxError::db("begin", e))?;
    let created = insert_within_limit(&txn, app.id, environment, owner).await?;
    txn.commit()
        .await
        .map_err(|e| SandboxError::db("commit", e))?;
    let actor = actor_label(db, Some(owner)).await;
    audit::record_best_effort(
        db,
        audit_entry(app, environment, actor, "app.environment.created")
            .actor(owner, audit::ActorType::User),
    )
    .await;
    Ok(created)
}

/// Under a lock on the app row: refuse a taken name and a full app, else
/// insert the row.
async fn insert_within_limit<C: ConnectionTrait>(
    txn: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    owner: Uuid,
) -> Result<app_environments::Model, SandboxError> {
    let app = apps::Entity::find_by_id(app_id)
        .lock_exclusive()
        .one(txn)
        .await
        .map_err(|e| SandboxError::db("lock the app", e))?
        .ok_or(SandboxError::AppNotFound)?;
    let name = environment.name();
    let sandboxes = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Kind.eq(AppEnvironmentKind::Dev.as_str()))
        .all(txn)
        .await
        .map_err(|e| SandboxError::db("read the app's sandboxes", e))?;
    if let Some(taken) = sandboxes.iter().find(|row| row.name == name) {
        return Err(match taken.deleting_at {
            Some(_) => SandboxError::Deleting(name),
            None => SandboxError::Exists(name),
        });
    }
    if sandboxes.len() as u64 >= MAX_SANDBOXES_PER_APP {
        return Err(SandboxError::Limit(MAX_SANDBOXES_PER_APP));
    }
    if let Some(owner) = sibling_owner(txn, &app, environment).await? {
        return Err(SandboxError::Reserved {
            name: name.clone(),
            app: owner,
        });
    }
    let now = Utc::now().fixed_offset();
    app_environments::ActiveModel {
        app_id: ActiveValue::Set(app_id),
        name: ActiveValue::Set(name),
        kind: ActiveValue::Set(AppEnvironmentKind::Dev.as_str().to_string()),
        owner_user_id: ActiveValue::Set(Some(owner)),
        build_id: ActiveValue::Set(None),
        updated_by: ActiveValue::Set(Some(owner)),
        updated_at: ActiveValue::Set(now),
        created_at: ActiveValue::Set(now),
        deleting_at: ActiveValue::Set(None),
    }
    .insert(txn)
    .await
    .map_err(|e| SandboxError::db("insert the sandbox", e))
}

/// The other app of the workspace whose own Airhouse schema carries the name
/// this sandbox's sibling would (`custom_apps_migrations::schema_owner`): a
/// sandbox must never be given a schema that is somebody's production data.
/// `None` when the sandbox has no sibling at all.
async fn sibling_owner<C: ConnectionTrait>(
    txn: &C,
    app: &apps::Model,
    environment: &AppEnvironment,
) -> Result<Option<String>, SandboxError> {
    let Some(home) = AirhouseHome::for_environment(&app.slug, environment)
        .ok()
        .flatten()
    else {
        return Ok(None);
    };
    schema_owner(txn, app.project_id, app.id, home.schema())
        .await
        .map_err(|e| SandboxError::db("read the workspace's app schemas", e))
}

/// Every environment of `app`: production, staging, then its sandboxes by
/// name — one being torn down included, as `status: "deleting"`.
pub async fn list(
    db: &DatabaseConnection,
    app: &apps::Model,
    org_slug: &str,
) -> Result<Vec<EnvironmentDto>, SandboxError> {
    let failed = |e: DbErr| SandboxError::db("read the app's environments", e);
    let rows = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app.id))
        .order_by_asc(app_environments::Column::Name)
        .all(db)
        .await
        .map_err(failed)?;
    // The fixed environments' builds as resolution answers them: the row, or
    // the legacy column where a row is missing.
    let fixed = load_environment_builds(db, app).await.map_err(failed)?;
    let view = View::load(db, app, org_slug, &rows, [fixed.production, fixed.staging])
        .await
        .map_err(failed)?;
    let row = |name: &str| rows.iter().find(|row| row.name == name);
    let mut environments = vec![
        view.fixed(
            &AppEnvironment::Production,
            row("production"),
            fixed.production,
        ),
        view.fixed(&AppEnvironment::Staging, row("staging"), fixed.staging),
    ];
    environments.extend(
        rows.iter()
            .filter(|row| is_sandbox(row))
            .filter_map(|row| view.sandbox(row)),
    );
    Ok(environments)
}

/// One environment of `app`. A sandbox nobody created, or one whose teardown
/// has finished, is `NotFound`; one still being torn down is shown.
pub async fn get(
    db: &DatabaseConnection,
    app: &apps::Model,
    org_slug: &str,
    environment: &AppEnvironment,
) -> Result<EnvironmentDto, SandboxError> {
    let name = environment.name();
    list(db, app, org_slug)
        .await?
        .into_iter()
        .find(|shown| shown.name == name)
        .ok_or(SandboxError::NotFound(name))
}

/// How an audit row names `actor`: their email, their id when they have none
/// (or cannot be read), and the expiry's own label when nobody asked.
pub(super) async fn actor_label(db: &DatabaseConnection, actor: Option<Uuid>) -> String {
    let Some(user_id) = actor else {
        return EXPIRY_ACTOR.to_string();
    };
    let email: Option<Option<String>> = entity::users::Entity::find_by_id(user_id)
        .select_only()
        .column(entity::users::Column::Email)
        .into_tuple()
        .one(db)
        .await
        .unwrap_or_default();
    email.flatten().unwrap_or_else(|| user_id.to_string())
}

pub(super) fn audit_entry(
    app: &apps::Model,
    environment: &AppEnvironment,
    actor: String,
    action: &'static str,
) -> audit::AuditEntry {
    audit::AuditEntry::new(actor, action)
        .org(app.org_id)
        .workspace(app.project_id)
        .target(
            "custom_app_environment",
            format!("{}/{environment}", app.id),
            format!("{}/{environment}", app.slug),
        )
        .environment(environment.name())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Production and staging are never created or deleted here; a sandbox
    /// passes whatever its handle.
    #[test]
    fn only_a_sandbox_is_created_or_deleted() {
        for fixed in [AppEnvironment::Production, AppEnvironment::Staging] {
            assert_eq!(
                require_sandbox(&fixed),
                Err(SandboxError::NotASandbox(fixed.name()))
            );
        }
        let sandbox = AppEnvironment::Dev {
            handle: "a1".into(),
        };
        assert_eq!(require_sandbox(&sandbox), Ok(()));
    }
}
