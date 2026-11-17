//! `app_publish_attempts`: one row per custom-app publish that reached the
//! pipeline, the ones that failed as well as the ones that landed a build
//! (`entity::app_publish_attempts`). `app_builds` records only a publish that
//! succeeded, so before this a failed publish left nothing behind.
//!
//! `app_id` cascades with its app, as the rest of the custom-apps aggregate
//! does. `app_build_id` is the build row a successful attempt recorded; it is
//! set NULL when retention deletes that build, so the attempt (and its stage
//! timings) outlive the bytes. The publisher columns mirror `app_builds`'
//! (`published_by`, `published_via`, `published_token_id`) and are loose:
//! a publish attempt is history, not a reference to keep alive.
//!
//! Additive and revert-safe: a binary one release back never reads or writes
//! the table, so it simply records no attempts.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS app_publish_attempts (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
    app_build_id UUID REFERENCES app_builds (id) ON DELETE SET NULL,
    build_id TEXT NOT NULL,
    environment TEXT NOT NULL,
    published_by UUID,
    published_via TEXT,
    published_token_id UUID,
    started_at TIMESTAMPTZ NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL,
    outcome TEXT NOT NULL CHECK (outcome IN ('success', 'failed')),
    failed_stage TEXT,
    error TEXT,
    stage_ms JSONB NOT NULL DEFAULT '{}'::jsonb
);
CREATE INDEX IF NOT EXISTS app_publish_attempts_app_started_idx
    ON app_publish_attempts (app_id, started_at DESC);
CREATE INDEX IF NOT EXISTS app_publish_attempts_app_build_idx
    ON app_publish_attempts (app_build_id);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS app_publish_attempts;
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
