use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// `app_events.source_app_id` may be null: an event the platform publishes
/// itself (a table change, app events phase 2) has no publishing app.
///
/// Shipped ahead of anything that writes a null, so this binary and every one
/// after it read a null `source_app_id` (the envelope names such an event's
/// source `/platform`), and reverting the release that first writes one lands
/// on a binary that already reads it. Nothing writes a null here.
const UP_SQL: &str = "ALTER TABLE app_events ALTER COLUMN source_app_id DROP NOT NULL;";

/// Restoring the constraint fails while a null row exists, which is the
/// point: those rows are the platform's own events, deleted by retention.
const DOWN_SQL: &str = "ALTER TABLE app_events ALTER COLUMN source_app_id SET NOT NULL;";

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
