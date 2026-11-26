//! OLTP change stream relay, platform side: `oltp_change_relays.wake_at`,
//! when a relay pass that left one of the store's baselines backing off
//! should run again. The sweep treats a mark as due once its `wake_at` has
//! passed, whatever its nudge and drain say, so a baseline is retried on a
//! store no write nudges; taking the relay's lease clears it.
//!
//! Additive and revert-safe: a nullable column a binary one release back
//! never reads.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE oltp_change_relays ADD COLUMN IF NOT EXISTS wake_at TIMESTAMPTZ";

const DOWN_SQL: &str = "ALTER TABLE oltp_change_relays DROP COLUMN IF EXISTS wake_at";

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
