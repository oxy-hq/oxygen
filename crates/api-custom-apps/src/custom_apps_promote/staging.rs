//! Moving **staging's** pointer by promote or rollback: a sandbox's build
//! promoted to staging (`dev-<handle>` → `staging`), or staging rolled back
//! (environments design §5.5a, rulings 1, 4 and 6).
//!
//! A pointer move like a draft publish's — staging's row, in one transaction
//! holding the app's row — whose event
//! is `staging / promote` or `staging / rollback`. That `promote` event is
//! what later admits a sandbox's build to production: it is staging having
//! served it. Staging's migrations are then **queued** exactly as a publish
//! queues them (`custom_apps_nonproduction::staging_task`), and never fail
//! the move: a failure there is its run's, reported as a warning. Staging's
//! own schedule rows are reconciled to the build it now serves, as a publish
//! to staging reconciles them (`custom_apps_env_schedules`): none while its
//! switch is off.

use entity::{app_builds, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{DatabaseConnection, TransactionTrait};
use uuid::Uuid;

use super::error::PromoteError;
use super::production::{describe, load_build, lock_app, refuse_agent_built};
use crate::custom_apps_env_schedules::reconcile::reconcile_environment;
use crate::custom_apps_environments::{self, EnvAction};
use crate::custom_apps_nonproduction::staging_task;
use crate::{custom_apps_cache, custom_apps_migrations as migrations};

/// One move of staging's pointer.
#[derive(Debug, Clone)]
pub struct StagingMove {
    pub app_id: Uuid,
    pub build: Uuid,
    /// `Promote` (from a sandbox) or `Rollback`.
    pub action: EnvAction,
    pub actor: Option<Uuid>,
    /// `Some(b)`: refuse `409 target_moved` unless staging still serves `b`.
    pub expect_current: Option<Option<Uuid>>,
}

/// Move staging's pointer under the app's row lock; never a build a sandbox
/// agent token published, never the build staging already serves.
pub async fn move_staging(
    db: &DatabaseConnection,
    mv: &StagingMove,
) -> Result<apps::Model, PromoteError> {
    let txn = db
        .begin()
        .await
        .map_err(|e| PromoteError::internal("begin the move", e))?;
    let row = lock_app(&txn, mv.app_id).await?;
    let build = load_build(&txn, mv.app_id, mv.build).await?;
    let current = custom_apps_environments::pointer(&txn, mv.app_id, &AppEnvironment::Staging)
        .await
        .map_err(|e| PromoteError::internal("read staging's pointer", e))?;
    check_pointer(current, &build, mv)?;
    refuse_agent_built(&txn, &build).await?;
    custom_apps_environments::record_move(
        &txn,
        mv.app_id,
        &AppEnvironment::Staging,
        Some(mv.build),
        mv.action,
        mv.actor,
    )
    .await
    .map_err(|e| PromoteError::internal("record staging's event", e))?;
    txn.commit()
        .await
        .map_err(|e| PromoteError::internal("commit the move", e))?;
    custom_apps_cache::invalidate_app_resolution_cache();
    Ok(row)
}

fn check_pointer(
    current: Option<Uuid>,
    build: &app_builds::Model,
    mv: &StagingMove,
) -> Result<(), PromoteError> {
    if let Some(expected) = mv.expect_current
        && expected != current
    {
        return Err(PromoteError::TargetMoved {
            environment: AppEnvironment::Staging.name(),
            expected: describe(expected),
            actual: describe(current),
        });
    }
    if current == Some(build.id) {
        return Err(PromoteError::AlreadyServing {
            environment: AppEnvironment::Staging.name(),
            build: build.build_id.clone(),
        });
    }
    Ok(())
}

/// Queue staging's migrations for the build it now serves, as a publish does.
/// The warnings are the move's; nothing here fails it.
pub async fn queue_staging_migrations(
    db: &DatabaseConnection,
    app: &apps::Model,
    build: &app_builds::Model,
) -> Vec<String> {
    let declared = match migrations::declared_in_build(build, &app.slug).await {
        Ok(declared) => declared,
        Err(e) => {
            tracing::warn!(app_id = %app.id, build = %build.build_id, error = %e,
                "promote: staging's migrations could not be read back");
            return vec![format!(
                "staging's migrations were not queued: the build's files could not be read ({e}); \
                 staging's writes there will fail until a later publish migrates it"
            )];
        }
    };
    let staged = staging_task::StagingBuild {
        app_id: app.id,
        app_slug: &app.slug,
        workspace_id: app.project_id,
        org_id: app.org_id,
        build_pk: build.id,
    };
    staging_task::queue_staging_migrations(db, staged, &declared.oltp, &declared.airhouse).await
}

/// Reconcile staging's schedule rows to the build it now serves. Best effort,
/// as a publish's is: a failure is the move's warning, never a failed move.
pub async fn reconcile_staging_schedules(
    db: &DatabaseConnection,
    app: &apps::Model,
) -> Vec<String> {
    let staging = AppEnvironment::Staging;
    reconcile_environment(db, app, &staging)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(app_id = %app.id, error = %e,
                "promote: staging's schedules were not reconciled");
            vec![format!(
                "staging's schedules could not be reconciled ({e}); publish to staging again, or \
                 flip its schedules switch, to retry"
            )]
        })
}

/// Reconcile staging's event subscriptions to the build it now serves. Best
/// effort, as its schedules are: a failure is the move's warning.
pub async fn reconcile_staging_subscriptions(
    db: &DatabaseConnection,
    app: &apps::Model,
) -> Vec<String> {
    crate::app_events::subscriptions::reconcile_environment(db, app, &AppEnvironment::Staging)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(app_id = %app.id, error = %e,
                "promote: staging's event subscriptions were not reconciled");
            vec![format!(
                "staging's event subscriptions could not be reconciled ({e}); publish to staging \
                 again to retry"
            )]
        })
}
