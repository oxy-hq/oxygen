//! Drop tables nothing reads or writes.
//!
//! * `a2a_tasks`, `a2a_messages`, `a2a_task_status`, `a2a_artifacts` — created in
//!   December 2025 and referenced by no code since. Messages and artifacts hold
//!   conversation content.
//! * `observability_spans`, `observability_intent_clusters`,
//!   `observability_intent_classifications`, `observability_metric_usage` — left
//!   behind when observability became ClickHouse-only. The ClickHouse store
//!   reuses these names, which is why they looked referenced; the Postgres copies
//!   hold prompts, SQL and user questions with their embeddings.
//! * `apalis_jobs` — the apalis queue, replaced by `agentic_task_queue`. No
//!   migration here ever created it, so it exists only where apalis created it
//!   itself; hence `IF EXISTS`.
//!
//! `down` is a no-op on purpose: nothing read these tables, so recreating them
//! empty would restore nothing a rollback could use.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const DEAD_TABLES: &[&str] = &[
    "a2a_artifacts",
    "a2a_messages",
    "a2a_task_status",
    "a2a_tasks",
    "observability_intent_classifications",
    "observability_intent_clusters",
    "observability_metric_usage",
    "observability_spans",
    "apalis_jobs",
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // One statement, so foreign keys among the dropped tables need no ordering
        // — and no CASCADE, which would silently take anything else that depends
        // on them.
        let mut drop = Table::drop();
        for table in DEAD_TABLES {
            drop.table(Alias::new(*table));
        }
        manager.drop_table(drop.if_exists().to_owned()).await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
