//! `custom_app_usage_backfill.parked_since` — the oldest usage row the drain
//! had to write to Postgres for a workspace that otherwise writes Airhouse
//! only, because that workspace's Airhouse append failed.
//!
//! Once a workspace's backfill has finished, its new page views and events go
//! to its Airhouse and not to Postgres (`oxy_custom_apps::usage_queue`). When an
//! append fails, the rows are written to Postgres instead, under the same ids,
//! so none is lost, and this column is set to the oldest of them (kept at the
//! oldest if it is already set). The backfill sweep queues a pass for every
//! finished workspace whose `parked_since` is set; that pass copies the
//! workspace's Postgres rows from `parked_since` on to Airhouse and clears the
//! column, unless more rows were parked while it ran.
//!
//! Additive and nullable: the previous release neither reads nor writes it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str =
    "ALTER TABLE custom_app_usage_backfill ADD COLUMN IF NOT EXISTS parked_since TIMESTAMPTZ;";

const DOWN_SQL: &str = "ALTER TABLE custom_app_usage_backfill DROP COLUMN IF EXISTS parked_since;";

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
