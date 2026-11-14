//! Workspace secrets shared with custom apps — `app_secret_grants`
//! (`internal-docs/customer-apps-functions.md`, "Workspace secrets shared with
//! an app").
//!
//! A grant is the one way an app reads a workspace secret: a row saying "app X
//! may read workspace secret Y", created by a workspace admin through the
//! `WorkspaceAdmin`-gated grant route — never by a bundle. `ctx.env` reads the
//! secret under its own name, so one stored value serves every app it is
//! shared with and a rotation reaches all of them at once.
//!
//! What a grant is **not**: a fallback. An app that declares or is shared
//! nothing sees nothing of the workspace, as before; the `apps/<app_id>/`
//! prefix still isolates one app's own secrets from every other app's, and an
//! app's own value of a key wins over a shared one.
//!
//! **Only a plain workspace secret can be shared** ([`is_shareable`]): never an
//! app's own `apps/<app_id>/<KEY>` row, so one app's secret can never be handed
//! to another this way.

use std::collections::BTreeSet;

use entity::app_secret_grants as grants;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QuerySelect,
    Set, TransactionTrait,
};
use uuid::Uuid;

use crate::secret_scope::{EnvEntry, EnvLookup, environment_segment};

/// Whether a stored secret name is a workspace secret that may be shared — a
/// plain name. An app's own row (`apps/<app_id>/<KEY>`, or an environment's
/// beneath it) never is.
pub fn is_shareable(name: &str) -> bool {
    !name.is_empty() && !name.contains('/')
}

/// The ids of the workspace secrets shared with `app_id`.
pub async fn shared_with_app<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
) -> Result<BTreeSet<Uuid>, DbErr> {
    Ok(grants::Entity::find()
        .select_only()
        .column(grants::Column::SecretId)
        .filter(grants::Column::AppId.eq(app_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await?
        .into_iter()
        .collect())
}

/// Every grant in the workspace, as `(secret_id, app_id)`.
pub async fn in_workspace<C: ConnectionTrait>(
    db: &C,
    project_id: Uuid,
) -> Result<Vec<(Uuid, Uuid)>, DbErr> {
    grants::Entity::find()
        .select_only()
        .column(grants::Column::SecretId)
        .column(grants::Column::AppId)
        .filter(grants::Column::ProjectId.eq(project_id))
        .into_tuple::<(Uuid, Uuid)>()
        .all(db)
        .await
}

/// What [`set_apps`] changed: the apps that gained the secret and the ones
/// that lost it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GrantChange {
    pub added: Vec<Uuid>,
    pub removed: Vec<Uuid>,
}

impl GrantChange {
    /// Every app whose `ctx.env` this change moved.
    pub fn touched(&self) -> impl Iterator<Item = Uuid> + '_ {
        self.added.iter().chain(self.removed.iter()).copied()
    }
}

/// Share `secret_id` with exactly `app_ids`: grant the ones it lacks, revoke
/// the rest, in one transaction.
///
/// The caller has already checked that the secret is a shareable workspace
/// secret of `project_id` and that every app belongs to that workspace — this
/// writes rows, it does not authorize them.
pub async fn set_apps(
    db: &DatabaseConnection,
    project_id: Uuid,
    secret_id: Uuid,
    app_ids: &BTreeSet<Uuid>,
    actor: Option<Uuid>,
) -> Result<GrantChange, DbErr> {
    let txn = db.begin().await?;
    let current: BTreeSet<Uuid> = grants::Entity::find()
        .select_only()
        .column(grants::Column::AppId)
        .filter(grants::Column::SecretId.eq(secret_id))
        .into_tuple::<Uuid>()
        .all(&txn)
        .await?
        .into_iter()
        .collect();
    let change = GrantChange {
        added: app_ids.difference(&current).copied().collect(),
        removed: current.difference(app_ids).copied().collect(),
    };
    if !change.removed.is_empty() {
        grants::Entity::delete_many()
            .filter(grants::Column::SecretId.eq(secret_id))
            .filter(grants::Column::AppId.is_in(change.removed.clone()))
            .exec(&txn)
            .await?;
    }
    if !change.added.is_empty() {
        let now = chrono::Utc::now().fixed_offset();
        let rows = change.added.iter().map(|app_id| grants::ActiveModel {
            id: Set(Uuid::new_v4()),
            project_id: Set(project_id),
            secret_id: Set(secret_id),
            app_id: Set(*app_id),
            created_at: Set(now),
            created_by: Set(actor),
        });
        // A concurrent writer granting the same pair loses to the unique key
        // quietly: the row it wanted exists either way.
        grants::Entity::insert_many(rows)
            .on_conflict_do_nothing_on([grants::Column::SecretId, grants::Column::AppId])
            .exec(&txn)
            .await?;
    }
    txn.commit().await?;
    Ok(change)
}

/// Stop sharing `secret_id` with every app — the workspace secret is being
/// deleted. The apps it was shared with, so their `ctx.env` can be dropped.
pub async fn revoke_all<C: ConnectionTrait>(db: &C, secret_id: Uuid) -> Result<Vec<Uuid>, DbErr> {
    let apps: Vec<Uuid> = grants::Entity::find()
        .select_only()
        .column(grants::Column::AppId)
        .filter(grants::Column::SecretId.eq(secret_id))
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    if !apps.is_empty() {
        grants::Entity::delete_many()
            .filter(grants::Column::SecretId.eq(secret_id))
            .exec(db)
            .await?;
    }
    Ok(apps)
}

/// The shared workspace secrets `ctx.env` adds to `plan` (the app's own keys,
/// [`crate::secret_scope::plan_env`]), each under its own name. Pure, so the
/// layering is testable without a database.
///
/// - **The app's own value wins**: a key `plan` already holds is not added.
/// - **Production reads every shared secret.**
/// - **Any other environment reads one only for a `shared` key**, exactly as
///   it reads production's own app secrets: a workspace secret is a production
///   credential, so staging and a sandbox never hold one they were not given
///   explicitly. Those entries are `through_fallback`, which is what makes
///   `ctx.secrets.set` refuse them there and a provider-sandbox `ctx.fetch`
///   carrying one held.
/// - **Nothing under [`EnvLookup::OwnOnly`]**, which reads the environment's
///   own path alone.
///
/// `granted` is the names of the active workspace secrets shared with the app;
/// a name that is not [`is_shareable`] is ignored rather than trusted.
pub fn plan_granted(
    plan: &[EnvEntry],
    environment: &AppEnvironment,
    lookup: EnvLookup,
    shared: &BTreeSet<String>,
    granted: &BTreeSet<String>,
) -> Vec<EnvEntry> {
    if lookup == EnvLookup::OwnOnly {
        return Vec::new();
    }
    let production = environment_segment(environment).is_none();
    granted
        .iter()
        .filter(|name| is_shareable(name))
        .filter(|name| production || shared.contains(*name))
        .filter(|name| !plan.iter().any(|own| own.key == **name))
        .map(|name| EnvEntry {
            key: name.clone(),
            stored_name: name.clone(),
            through_fallback: !production,
        })
        .collect()
}

#[cfg(test)]
#[path = "secret_grants_tests.rs"]
mod tests;
