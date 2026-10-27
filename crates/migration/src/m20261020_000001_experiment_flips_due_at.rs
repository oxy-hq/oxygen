use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// An hour-grain experiment switches at any whole local hour, so a flip is due at a
/// moment, not on a day: `experiment_flips.due_on` (date) becomes `due_at` (a local
/// `timestamp`, in `design.timezone`), `confirmed_on` keeps the hour read-back saw
/// the lever land, and `experiment_runs.closing_since` keeps the hour the run
/// entered closing. Existing rows become midnight of their date, which
/// is what they meant. Both unique keys follow the renamed column. Nothing deployed
/// ever read these columns (see migration_rollback_safety.rs, `DECLARED`).
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "ALTER TABLE experiment_flips RENAME COLUMN due_on TO due_at; \
             ALTER TABLE experiment_flips ALTER COLUMN due_at TYPE timestamp USING due_at::timestamp; \
             ALTER TABLE experiment_flips ALTER COLUMN confirmed_on TYPE timestamp \
               USING confirmed_on::timestamp; \
             ALTER TABLE experiment_runs ALTER COLUMN closing_since TYPE timestamp \
               USING closing_since::timestamp",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared(
            "ALTER TABLE experiment_runs ALTER COLUMN closing_since TYPE date \
               USING closing_since::date; \
             ALTER TABLE experiment_flips ALTER COLUMN confirmed_on TYPE date USING confirmed_on::date; \
             ALTER TABLE experiment_flips ALTER COLUMN due_at TYPE date USING due_at::date; \
             ALTER TABLE experiment_flips RENAME COLUMN due_at TO due_on",
        )
        .await?;
        Ok(())
    }
}
