//! `app_function_invocations.parent_invocation_id` — the invocation that made
//! this one, for a run started by `ctx.apps.call` (`mode = 'call'`): the
//! caller's invocation. NULL for every other run, and for every row written
//! before this column.
//!
//! Expand-only: one nullable column, no default, no index, no constraint, no
//! backfill, and no foreign key (the parent may be pruned before the child; the
//! link is a pointer for the console and `oxyc`, not an ownership). A binary one
//! release back never reads it and writes it as NULL.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str =
    "ALTER TABLE app_function_invocations ADD COLUMN IF NOT EXISTS parent_invocation_id UUID;";

const DOWN_SQL: &str =
    "ALTER TABLE app_function_invocations DROP COLUMN IF EXISTS parent_invocation_id;";

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
