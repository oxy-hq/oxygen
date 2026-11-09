//! Fixed-window rate-limit counters shared by every replica
//! (`oxy_app_core::rate_limit`): one row per `(key, window_start)`, counted
//! with one atomic upsert. `expires_at` is the window's own end, so cleanup is
//! one `DELETE … WHERE expires_at < now - slack` whatever the window length.
//!
//! Additive and revert-safe: a binary one release back never reads the table,
//! and its in-process limiters keep working without it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS rate_limit_windows (
    key TEXT NOT NULL,
    window_start TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    count BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (key, window_start)
);
CREATE INDEX IF NOT EXISTS rate_limit_windows_expires_at_idx ON rate_limit_windows (expires_at);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS rate_limit_windows;
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
