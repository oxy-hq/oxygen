//! A failed lever flip is retried: `attempts` counts its sends and `failed_at` keeps
//! the local hour (in `design.timezone`, like `due_at`) its last send failed, so the
//! run's state machine can back off between attempts and cap an `on`'s retries.
//!
//! Expand-only: two columns, one defaulted and one nullable. A binary one release
//! back never reads them and its inserts take the default.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
ALTER TABLE experiment_flips ADD COLUMN IF NOT EXISTS attempts INTEGER NOT NULL DEFAULT 1;
ALTER TABLE experiment_flips ADD COLUMN IF NOT EXISTS failed_at TIMESTAMP;
"#;

const DOWN_SQL: &str = r#"
ALTER TABLE experiment_flips DROP COLUMN IF EXISTS failed_at;
ALTER TABLE experiment_flips DROP COLUMN IF EXISTS attempts;
"#;

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
