//! `app_events.invoker` and `app_events.invoker_token_id` — who a
//! `ctx.apps.send` ran as, when its run ran directly as a person: `session`
//! (a login session) or `token` with the stored token's id. The relay writes
//! them only for a send whose outbox row carries an invoker MAC that verifies
//! (`app_events::provenance`, message domain v2), and a delivery re-reads that
//! credential to run the send as that person under the same ceiling. NULL for
//! every other event, and for every row written before these columns: those
//! sends run as the system, as every send did.
//!
//! Expand-only: two nullable columns, no default, no index, no constraint, no
//! backfill, and no foreign key (a token is revoked, never deleted, and a
//! delivery refuses one it cannot re-admit). A binary one release back never
//! reads them and writes them as NULL.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE app_events ADD COLUMN IF NOT EXISTS invoker TEXT; \
     ALTER TABLE app_events ADD COLUMN IF NOT EXISTS invoker_token_id UUID;";

const DOWN_SQL: &str = "ALTER TABLE app_events DROP COLUMN IF EXISTS invoker_token_id; \
     ALTER TABLE app_events DROP COLUMN IF EXISTS invoker;";

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
