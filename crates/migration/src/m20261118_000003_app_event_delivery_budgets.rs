use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// Where a redelivered delivery's attempt budget restarts (app events design
/// §6.8): the attempts it had made when an engineer redelivered it. A
/// delivery may make that many plus its listener's `maxAttempts`, so a dead
/// delivery redelivered runs again rather than dying at once, with every past
/// attempt row and its id kept. One row per delivery, replaced by each
/// redelivery; `restarted_at` is when it last became pending.
const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS app_event_delivery_budgets (
    delivery_id UUID PRIMARY KEY REFERENCES app_event_deliveries (id) ON DELETE CASCADE,
    attempts_before INTEGER NOT NULL,
    restarted_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
"#;

const DOWN_SQL: &str = "DROP TABLE IF EXISTS app_event_delivery_budgets;";

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
