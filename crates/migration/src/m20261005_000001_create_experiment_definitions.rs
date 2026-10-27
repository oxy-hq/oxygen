//! `experiment_definitions` (`*.experiment.yml`, compiled) — see
//! `internal-docs/experiments.md` "The artifact" and index contract C3.
//!
//! Its own migration, not an append to the squashed compile-boundary one: SeaORM
//! skips `up()` for a name it has already recorded, so an appended table would
//! never reach a database that applied the original (same reasoning as
//! `m20260812_000001_create_simulation_definitions`).
//!
//! Additive and idempotent, per `internal-docs/revert-safe-migrations.md`: one
//! `CREATE TABLE IF NOT EXISTS`, nothing dropped, renamed or tightened, so an
//! older binary lives with it and a roll-forward re-run is a no-op.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS experiment_definitions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL,
    -- Cascade like every compile-boundary table: dropping a revision drops its rows.
    revision_id UUID NOT NULL REFERENCES revisions(revision_id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    config JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- The compile step reports a duplicate name as a diagnostic naming both
    -- files; this is the backstop that keeps the database itself honest.
    CONSTRAINT experiment_definitions_revision_name_key UNIQUE (revision_id, name)
);
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
            .execute_unprepared("DROP TABLE IF EXISTS experiment_definitions;")
            .await?;
        Ok(())
    }
}
