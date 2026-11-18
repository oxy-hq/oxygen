use sea_orm_migration::prelude::*;

/// `app_environments.events_enabled`: whether a non-production environment of
/// a custom app receives published events (app events design §6.6).
///
/// Off by default, for staging and sandboxes alike: an event run outside
/// production calls a function with another app's payload, so an environment
/// takes events only once staff turned it on. Production has no switch and
/// never reads the column.
///
/// Control-plane state about an environment — no row of an org's data.
///
/// **Rollback-safe**: one column with a constant default, no constraint on
/// the existing table. The previous deploy never reads or writes it, and a
/// row it inserts takes the default — off.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE app_environments \
     ADD COLUMN IF NOT EXISTS events_enabled BOOLEAN NOT NULL DEFAULT false";

const DOWN_SQL: &str = "ALTER TABLE app_environments DROP COLUMN IF EXISTS events_enabled";

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
