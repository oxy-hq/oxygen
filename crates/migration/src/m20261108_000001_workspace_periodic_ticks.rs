//! `workspace_periodic_ticks` — when each workspace is next due to be driven
//! by the periodic global driver, and which compiled revision its schedule
//! rows were last reconciled from.
//!
//! One row per workspace, and `next_tick_at` is the coordination point: a
//! node claims a batch of due rows with `FOR UPDATE SKIP LOCKED` and moves
//! each one interval out in the same statement, so however many nodes tick,
//! each workspace is driven by exactly one of them per interval. Before this
//! table every node loaded every workspace on every tick and built a project
//! context for each; the losers' work was wasted and the cost grew as
//! workspaces × nodes (`internal-docs/worker-fleet.md`, "The periodic driver
//! claims workspaces").
//!
//! A table of its own, mirroring `workspace_compile_checks`, rather than two
//! columns on `workspaces`: the claim rewrites its row on every interval on
//! every workspace, and `workspaces` is the busiest row in the database —
//! read by every request's middleware — so a row lock and a dead tuple per
//! tick there would be paid by every reader for the sake of one loop. Nor a
//! row in `agentic_schedules`: that table is the user-facing Schedules
//! surface, whose tick and "run now" would both have to learn to skip a
//! system row.
//!
//! Control plane only: ids and timestamps. Purely additive — a new table
//! nothing older reads or writes, so a reverted binary ignores it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS workspace_periodic_ticks (
    workspace_id UUID PRIMARY KEY REFERENCES workspaces(id) ON DELETE CASCADE,
    next_tick_at TIMESTAMPTZ NOT NULL,
    claimed_at TIMESTAMPTZ,
    last_reconciled_revision_id UUID,
    last_reconciled_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_workspace_periodic_ticks_due
    ON workspace_periodic_ticks (next_tick_at);
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
            .execute_unprepared("DROP TABLE IF EXISTS workspace_periodic_ticks;")
            .await?;
        Ok(())
    }
}
