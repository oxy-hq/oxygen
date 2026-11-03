use sea_orm_migration::prelude::*;

/// `app_environments.semantic_revision_id`, `semantic_git_sha`: the semantic
/// pin of a custom-app environment (`internal-docs/platform-roadmap.md`,
/// custom-app branches, the workspace planes).
///
/// - `semantic_revision_id` is the compiled revision the environment's data
///   plane reads when the build it serves pins nothing of its own. The
///   environment is served that revision while it is `ready`, even after
///   `semantic_git_sha` has moved on to a newer commit still compiling.
/// - `semantic_git_sha` is the commit the environment's semantic branch
///   currently points at. Set while no ready revision is pinned, it is what a
///   refusal names: the plane's home is not ready, and the environment is
///   refused rather than routed to another model.
///
/// Control-plane state about an environment — no row of an org's data.
///
/// **No foreign key, deliberately**, for the reason `app_builds.
/// semantic_revision_id` has none: adding one to an existing table is a
/// contraction under `migration_rollback_safety`, and nothing needs it.
/// Retention (`compile_maintenance::REVISION_IN_USE`) keeps a pinned revision,
/// and `custom_apps_staging_pin::environment_pin` joins `revisions`, so a
/// dangling id reads as "no ready revision pinned".
///
/// **Rollback-safe**: two nullable columns on an existing table, no constraint
/// or index. The previous deploy never reads or writes them, and a row it
/// inserts leaves both null, which reads as "no environment pin".
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE app_environments \
     ADD COLUMN IF NOT EXISTS semantic_revision_id UUID, \
     ADD COLUMN IF NOT EXISTS semantic_git_sha TEXT";

const DOWN_SQL: &str = "ALTER TABLE app_environments \
     DROP COLUMN IF EXISTS semantic_revision_id, \
     DROP COLUMN IF EXISTS semantic_git_sha";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
