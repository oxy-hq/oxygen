use sea_orm_migration::prelude::*;

/// `app_environments.deleting_at`: the marker of a sandbox being torn down
/// (`internal-docs/custom-app-sandboxes.md` → Lifecycle).
///
/// Deleting a sandbox is two-phase. The request sets `deleting_at` and clears
/// the build pointer in one transaction, so the sandbox stops serving at once;
/// a queued task then removes its storage silo, its secrets and its Airhouse
/// sibling, and only then the row. Without a marker, re-creating the name
/// would race that teardown and either inherit the old homes or lose its new
/// ones to it. With it, the name stays taken until the row is gone.
///
/// `NULL` for every existing row and for `production` and `staging` always:
/// only a `dev` row is ever marked.
///
/// **Rollback-safe**: additive and nullable. The previous deploy never reads
/// or writes the column, and never resolves a `dev` row at all.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE app_environments ADD COLUMN IF NOT EXISTS deleting_at TIMESTAMPTZ",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("ALTER TABLE app_environments DROP COLUMN IF EXISTS deleting_at")
            .await?;
        Ok(())
    }
}
