//! Index `agentic_task_queue.run_id`.
//!
//! The column had none: the table's indexes were the primary key, the two
//! partial poll/reap indexes, the compile backoff index and (from the
//! migration before this one) `root_task_id`. Yet it is the join key from a
//! run to its queue rows — the workspace queue stats and the staff
//! internal-jobs listings (`r.id = q.run_id`), `mark_released_roots_global`,
//! airway's root-row lookup, the compile status read, workspace health — and
//! the foreign key `ON DELETE CASCADE` from `agentic_runs`, which without it
//! scans the queue once per deleted run.
//!
//! A plain `CREATE INDEX` in the migrator's transaction; see
//! `task_queue_root_task_id` for why not `CONCURRENTLY`.

use sea_orm_migration::prelude::*;

pub struct AddTaskQueueRunIdIndex;

/// Named by hand: `DeriveMigrationName` takes the module path (see
/// `AddCompileTaskBackoffIndex`).
impl MigrationName for AddTaskQueueRunIdIndex {
    fn name(&self) -> &str {
        "m20261105_000002_add_task_queue_run_id_index"
    }
}

pub const RUN_ID_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_task_queue_run_id \
     ON agentic_task_queue (run_id)";

#[async_trait::async_trait]
impl MigrationTrait for AddTaskQueueRunIdIndex {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(RUN_ID_INDEX_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_task_queue_run_id")
            .await?;
        Ok(())
    }
}
