use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // What an abort does with the levers (`off` | `keep`): the owner's choice with the
        // reason, then what the abort did once the run is aborted (a guardrail always turns
        // them off). And the day it was aborted, which places the measurement cut when no
        // `off` was sent. Both nullable: an existing run reads as turned off, with its cut
        // still taken from its `off` flips, and an older binary never selects them
        // (revert-safe-migrations.md).
        manager
            .alter_table(
                Table::alter()
                    .table(ExperimentRuns::Table)
                    .add_column_if_not_exists(ColumnDef::new(ExperimentRuns::AbortLevers).text())
                    .add_column_if_not_exists(ColumnDef::new(ExperimentRuns::AbortedOn).date())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ExperimentRuns::Table)
                    .drop_column(ExperimentRuns::AbortLevers)
                    .drop_column(ExperimentRuns::AbortedOn)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ExperimentRuns {
    Table,
    AbortLevers,
    AbortedOn,
}
