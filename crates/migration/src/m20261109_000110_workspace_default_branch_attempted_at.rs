//! Adds `workspaces.default_branch_attempted_at`: when the periodic compile
//! check last asked GitHub for the workspace's default branch and got no answer
//! it could record (`compile_reconcile/default_branch.rs`). The recorder skips a
//! workspace asked within the hour and takes the least recently asked first, so
//! a few workspaces GitHub cannot answer for (a repository gone, a token
//! refused) do not starve the rest or re-ask GitHub on every tick.
//!
//! Control-plane bookkeeping about the workspace, not org data. Nullable with
//! no default, so a binary from before this migration, which names its
//! columns, keeps reading and inserting. Guarded with `has_column`.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[derive(DeriveIden)]
enum Workspaces {
    Table,
    DefaultBranchAttemptedAt,
}

const TABLE: &str = "workspaces";
const COLUMN: &str = "default_branch_attempted_at";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !manager.has_column(TABLE, COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Workspaces::Table)
                        .add_column(
                            ColumnDef::new(Workspaces::DefaultBranchAttemptedAt)
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
        if manager.has_column(TABLE, COLUMN).await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(Workspaces::Table)
                        .drop_column(Workspaces::DefaultBranchAttemptedAt)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}
