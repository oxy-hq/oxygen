//! `agentic_task_queue.retry_attempt`: how many of its declared retries a
//! task has spent.
//!
//! A retry is the same row going `claimed -> queued` again behind its backoff
//! (`crud::retry`), so the count has to live on the row: `claim_count` is the
//! crash budget and restarts with every attempt, and `claim_epoch` also counts
//! reaper re-claims. Revert-safe: old code neither reads nor writes it, and the
//! default makes every existing row a first attempt.

use sea_orm_migration::prelude::*;

use super::column_exists;

pub struct AddTaskQueueRetryAttempt;

/// Named by hand: `DeriveMigrationName` takes the module path (see
/// `AddCompileTaskBackoffIndex`).
impl MigrationName for AddTaskQueueRetryAttempt {
    fn name(&self) -> &str {
        "m20261022_000001_add_task_queue_retry_attempt"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddTaskQueueRetryAttempt {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_task_queue", "retry_attempt").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    "ALTER TABLE agentic_task_queue \
                     ADD COLUMN retry_attempt INTEGER NOT NULL DEFAULT 0",
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_task_queue", "retry_attempt").await? {
            manager
                .get_connection()
                .execute_unprepared("ALTER TABLE agentic_task_queue DROP COLUMN retry_attempt")
                .await?;
        }
        Ok(())
    }
}
