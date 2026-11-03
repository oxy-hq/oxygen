use sea_orm_migration::prelude::*;

/// `app_environments.parent_name`, `fork_at`, `planes`: the branch facts of a
/// custom-app environment (`internal-docs/platform-roadmap.md`, custom-app
/// branches).
///
/// - `parent_name` is the environment this one was cut from. Null reads as the
///   kind's default parent (a sandbox's is `staging`, staging's is
///   `production`, production has none).
/// - `fork_at` is the point on the parent this environment was cut at. Null
///   reads as "when the environment was created".
/// - `planes` holds the per-plane mode choices (`{}` means every plane uses
///   its default for the kind). It is never null.
///
/// Control-plane state about an environment — no row of an org's data. P0 only
/// adds the columns; nothing writes them yet.
///
/// **Rollback-safe**: additive only. Two nullable columns and one with a
/// constant default, and no constraint, foreign key or index on the existing
/// table. The previous deploy never reads or writes them, and a row it inserts
/// takes the defaults, which read as "the kind's default" everywhere.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE app_environments \
     ADD COLUMN IF NOT EXISTS parent_name TEXT, \
     ADD COLUMN IF NOT EXISTS fork_at TIMESTAMPTZ, \
     ADD COLUMN IF NOT EXISTS planes JSONB NOT NULL DEFAULT '{}'::jsonb";

const DOWN_SQL: &str = "ALTER TABLE app_environments \
     DROP COLUMN IF EXISTS parent_name, \
     DROP COLUMN IF EXISTS fork_at, \
     DROP COLUMN IF EXISTS planes";

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
