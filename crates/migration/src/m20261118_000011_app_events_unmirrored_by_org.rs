use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The event log mirror reads the oldest unmirrored events of the orgs that
/// have `app_events` on (`app_events::mirror`). Led by `org_id`, an index range
/// per such org gives `relayed_at` order without walking events of orgs that
/// have the flag off, which stay unmirrored until retention deletes them.
/// Additive; `app_events` is the platform's routing table, small enough for a
/// plain (non-concurrent) build.
const UP_SQL: &str = "CREATE INDEX IF NOT EXISTS app_events_unmirrored_by_org_idx \
    ON app_events (org_id, relayed_at, id) WHERE mirrored_at IS NULL;";

const DOWN_SQL: &str = "DROP INDEX IF EXISTS app_events_unmirrored_by_org_idx;";

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
