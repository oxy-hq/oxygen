use sea_orm_migration::prelude::*;

/// `app_environments.airhouse_pinned_at`: the point in time a custom-app
/// environment whose Airhouse plane is `snapshot` reads production at
/// (`internal-docs/2026-11-02-custom-app-branches-design.md` §5.4).
///
/// Null until the environment's first `snapshot` read pins it, or until a
/// reset of its Airhouse plane re-pins it. Every other mode never reads it.
/// It is its own column, not `fork_at`, because a reset of one plane must not
/// move the fork point the other planes were cut at.
///
/// Control-plane state about an environment — a timestamp, no row of an
/// org's data.
///
/// **Rollback-safe**: additive only. One nullable column, no constraint,
/// foreign key or index. The previous deploy never reads or writes it.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE app_environments \
     ADD COLUMN IF NOT EXISTS airhouse_pinned_at TIMESTAMPTZ";

const DOWN_SQL: &str = "ALTER TABLE app_environments \
     DROP COLUMN IF EXISTS airhouse_pinned_at";

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
