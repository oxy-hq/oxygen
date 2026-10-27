//! `agentic_runs.notify_requested_at`: someone asked to hear when the run ends
//! (`lifecycle::notice_request`).
//!
//! **Revert-safe.** One nullable column, added only if missing and with no
//! default to backfill: a binary that predates it never names it, so it reads
//! and writes the table as before, and the ledger row it does not know is
//! tolerated (`internal-docs/revert-safe-migrations.md`). `down` drops it.

use sea_orm_migration::prelude::*;

use super::column_exists;

pub struct AddRunNotifyRequested;

/// Named by hand: `DeriveMigrationName` takes the module path (see
/// `AddCompileTaskBackoffIndex`).
impl MigrationName for AddRunNotifyRequested {
    fn name(&self) -> &str {
        "m20261022_000001_add_run_notify_requested"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddRunNotifyRequested {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_runs", "notify_requested_at").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("agentic_runs"))
                        .add_column(
                            ColumnDef::new(Alias::new("notify_requested_at"))
                                .timestamp_with_time_zone()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_runs", "notify_requested_at").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Alias::new("agentic_runs"))
                        .drop_column(Alias::new("notify_requested_at"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
