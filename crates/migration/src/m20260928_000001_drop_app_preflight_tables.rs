//! Drop the two tables the blocking custom-app preflight kept.
//!
//! * `app_preflight_refusals` — the refusals each rollout it let through had
//!   seen (`m20260925_000001`).
//! * `app_preflight_blocks` — how many times a blocked rollout had retried
//!   (`m20260925_000003`).
//!
//! The preflight became report-only in #3362 and stopped reading and writing
//! both; that shipped to prod in 0.5.153 (build `b7f49d7`). This is phase 2.
//! An older binary that still has the blocking preflight fails soft without
//! them: it logs "could not run" and never blocks.
//!
//! `down` is a no-op on purpose: nothing reads these tables, so recreating
//! them empty would restore nothing a rollback could use.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(Alias::new("app_preflight_blocks"))
                    .table(Alias::new("app_preflight_refusals"))
                    .if_exists()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        Ok(())
    }
}
