//! Indexes for the run-retention prune (oxy-app's `server::router::run_retention`).
//!
//! * [`RUN_RETENTION_INDEX_SQL`]: finished root runs by age. The prune walks
//!   them oldest first; without it, every pass read the whole table to find
//!   the few hundred past the window.
//! * [`RUN_THREAD_INDEX_SQL`]: a thread's runs by recency. The prune keeps a
//!   thread whole while any of its runs is recent, which is a probe per
//!   candidate. Thread history (`get_runs_by_thread`) had no index on
//!   `thread_id` either and reads through this one too.
//!
//! **Frozen DDL, coupled to a query.** A partial index is used only when the
//! query implies its predicate, so the prune spells the same status list. When
//! they disagree, the fix is a NEW migration, never an edit to this one.

use sea_orm_migration::prelude::*;

pub const RUN_RETENTION_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_agentic_runs_retention \
     ON agentic_runs (updated_at) \
     WHERE parent_run_id IS NULL \
       AND task_status IN ('done', 'failed', 'cancelled', 'timed_out')";

pub const RUN_THREAD_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_agentic_runs_thread_updated \
     ON agentic_runs (thread_id, updated_at) \
     WHERE thread_id IS NOT NULL";

pub struct AddRunRetentionIndexes;

/// Named by hand: `DeriveMigrationName` takes the module path (see
/// `AddCompileTaskBackoffIndex`).
impl MigrationName for AddRunRetentionIndexes {
    fn name(&self) -> &str {
        "m20261109_000001_add_run_retention_indexes"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddRunRetentionIndexes {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(RUN_RETENTION_INDEX_SQL).await?;
        db.execute_unprepared(RUN_THREAD_INDEX_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_agentic_runs_thread_updated")
            .await?;
        db.execute_unprepared("DROP INDEX IF EXISTS idx_agentic_runs_retention")
            .await?;
        Ok(())
    }
}
