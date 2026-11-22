//! Removing the fixture: `oxy dev seed --clear`, and a rebuild of a fixture that
//! is partial or out of date (a crash mid-way, or an edited bundle file).
//!
//! Only the app with the fixture's derived id is touched; a developer's own
//! `local/pulse-demo` has another id and is left alone. Order matters:
//!
//! 1. **Schedules first.** Production's and staging's function schedules are
//!    rows keyed by the string `<app_id>/<function>` (or `<app_id>/<env>/…`),
//!    which no foreign key reaches: deleting the app would leave `snapshot`
//!    and `digest` firing at an app that no longer exists. Each is reconciled
//!    to nothing.
//! 2. **Bytes, then the row.** The bundle bytes and the asset silo, then the
//!    `apps` row, whose foreign keys cascade to the builds, functions,
//!    invocations, events and environments.

use entity::apps;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, EntityTrait};

use super::FixtureError;
use crate::custom_apps_env_schedules::reconcile::{reconcile_production, retire_environment};
use crate::{custom_apps_build_store, custom_apps_cache, custom_apps_storage};

/// Remove `app` (already proven to be the fixture's). Answers whether a row
/// was deleted.
pub async fn remove(db: &DatabaseConnection, app: &apps::Model) -> Result<bool, FixtureError> {
    let warnings = reconcile_production(db, app.id, app.project_id, &[]).await;
    if let Some(first) = warnings.into_iter().next() {
        return Err(FixtureError::step("retire production's schedules", first));
    }
    retire_environment(db, app.project_id, app.id, &AppEnvironment::Staging)
        .await
        .map_err(|e| FixtureError::step("retire staging's schedules", e))?;
    custom_apps_build_store::delete_app(app.id)
        .await
        .map_err(|e| FixtureError::step("delete the bundle bytes", e))?;
    custom_apps_storage::delete_app_assets(app.id)
        .await
        .map_err(|e| FixtureError::step("delete the asset silo", e))?;
    let deleted = apps::Entity::delete_by_id(app.id)
        .exec(db)
        .await
        .map_err(|e| FixtureError::step("delete the app row", e))?
        .rows_affected;
    custom_apps_cache::invalidate_app_resolution_cache();
    Ok(deleted > 0)
}
