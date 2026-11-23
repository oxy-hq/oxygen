//! The react loop's per-user memory (`react::memory`): one row per write to a
//! user's notes in one workspace. A note is the latest `put` for its
//! `note_id`; a `delete` row removes it. Rows are only appended, and `id`
//! keeps their order, which `react::memory::latest` breaks ties on (a merge
//! appends many rows with one timestamp).
//!
//! Org content in Oxy's database, beside the conversation it was learned from
//! (`agentic_runs`): `data_placement.rs` lists it as backlog that moves with
//! them to the org's OLTP. A deliberate exception to "data plane from day
//! one", signed off by the repo owner on 2026-11-23 (#4382): most orgs have
//! no OLTP store, and notes move with the chats, not ahead of them. No foreign keys, as for the runs: this migrator
//! runs apart from the central one.

use sea_orm_migration::prelude::*;

const AGENT_MEMORY_NOTES_SQL: &str = "CREATE TABLE IF NOT EXISTS agent_memory_notes ( \
     id bigserial PRIMARY KEY, \
     workspace_id uuid NOT NULL, \
     user_id uuid NOT NULL, \
     note_id text NOT NULL, \
     op text NOT NULL CHECK (op IN ('put', 'delete')), \
     text text NOT NULL DEFAULT '', \
     run_id text NOT NULL, \
     revision_id uuid NULL, \
     recorded_at timestamptz NOT NULL )";

const AGENT_MEMORY_NOTES_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS \
     idx_agent_memory_notes_owner ON agent_memory_notes (workspace_id, user_id, id)";

pub struct CreateAgentMemoryNotes;

/// Named by hand: `DeriveMigrationName` takes the module path (see
/// `AddCompileTaskBackoffIndex`).
impl MigrationName for CreateAgentMemoryNotes {
    fn name(&self) -> &str {
        "m20261123_000001_create_agent_memory_notes"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateAgentMemoryNotes {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(AGENT_MEMORY_NOTES_SQL).await?;
        db.execute_unprepared(AGENT_MEMORY_NOTES_INDEX_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP TABLE IF EXISTS agent_memory_notes")
            .await?;
        Ok(())
    }
}
