//! `custom_app_usage_purges`: the ledger of usage-fact purges a deleted custom
//! app still owes its workspace's Airhouse (`entity::custom_app_usage_purges`).
//!
//! A purge was only ever a row in `agentic_task_queue`, and queue retention
//! prunes failed rows after 30 days, so an outage that outlasted the sweep's
//! retries left no record of the debt. This table is the record: written in the
//! transaction that deletes the app, closed (`completed_at`) when Airhouse has
//! deleted the facts, parked (`abandoned_at`) when the retries run out, and
//! never pruned while it is owed.
//!
//! No foreign key to `apps`: the row exists precisely because the app is gone.
//! Control plane (ids, a truncated error; no usage content).
//!
//! Additive and revert-safe: a new table only, which a binary one release back
//! never reads.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS custom_app_usage_purges (
    app_id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL,
    org_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL,
    last_error TEXT,
    completed_at TIMESTAMPTZ,
    abandoned_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS custom_app_usage_purges_owed_idx
    ON custom_app_usage_purges (next_attempt_at)
    WHERE completed_at IS NULL AND abandoned_at IS NULL;
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS custom_app_usage_purges;
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
