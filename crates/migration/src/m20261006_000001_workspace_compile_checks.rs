//! `workspace_compile_checks` — when each remote-backed workspace is next due
//! to have its default branch's head compared with the revision it serves.
//!
//! One row per workspace, and `next_check_at` is the coordination point: a
//! replica claims a check with a conditional UPDATE on the value it read, so
//! however many replicas tick, one of them asks GitHub. The other columns are
//! what the last check found, for an operator reading the table.
//!
//! A table of its own rather than a column on `workspaces` or a row in
//! `agentic_schedules`: that schedule table is the user-facing Schedules
//! surface, with a generic tick and a "run now" that would both have to learn
//! to skip a system row; and a column on `workspaces` is read by every query
//! of the busiest entity for the sake of one periodic loop.
//!
//! Control plane only: ids, timestamps and commit SHAs. Purely additive — a
//! new table nothing older reads or writes.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS workspace_compile_checks (
    workspace_id UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    next_check_at TIMESTAMPTZ NOT NULL,
    last_checked_at TIMESTAMPTZ,
    last_head_sha TEXT,
    last_outcome TEXT
);
CREATE INDEX IF NOT EXISTS idx_workspace_compile_checks_due
    ON workspace_compile_checks (next_check_at);
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
            .execute_unprepared("DROP TABLE IF EXISTS workspace_compile_checks;")
            .await?;
        Ok(())
    }
}
