//! Per-token daily usage. Design:
//! `internal-docs/2026-09-30-api-tokens-design.md` §3.7 and §7.
//!
//! Expand-only: one new table. Nothing an older binary reads or writes
//! changes, so a revert leaves it working as before (it simply never writes
//! usage).
//!
//! `api_token_usage_daily` holds one row per token per UTC day: counts, and
//! the last IP, user agent and **route template** seen. Ids and counters, no
//! request content. Rows go when their token goes.
//!
//! The two `audit_events` indexes that answer "what did this token do?" are
//! **not** here. That table is hot, so they are built `CONCURRENTLY`, which
//! cannot share this migration's transaction:
//! `m20261005_000001_audit_events_token_indexes`. The index below is on the
//! table this migration creates, which is empty and unread until it commits.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS api_token_usage_daily (
    token_id UUID NOT NULL REFERENCES api_tokens(id) ON DELETE CASCADE,
    day DATE NOT NULL,
    requests BIGINT NOT NULL DEFAULT 0,
    errors_4xx BIGINT NOT NULL DEFAULT 0,
    errors_5xx BIGINT NOT NULL DEFAULT 0,
    last_ip TEXT,
    last_user_agent TEXT,
    last_route TEXT,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (token_id, day)
);
-- The retention prune deletes by day alone.
CREATE INDEX IF NOT EXISTS idx_api_token_usage_daily_day
    ON api_token_usage_daily (day);
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
            .execute_unprepared("DROP TABLE IF EXISTS api_token_usage_daily")
            .await?;
        Ok(())
    }
}
