use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // The target measure day by day across the run, sealed with the result. Nullable
        // and additive: a row written before it reads `NULL` and renders without the
        // series, and an older binary never selects it (revert-safe-migrations.md).
        manager
            .alter_table(
                Table::alter()
                    .table(ExperimentResults::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(ExperimentResults::Series)
                            .json_binary()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ExperimentResults::Table)
                    .drop_column(ExperimentResults::Series)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ExperimentResults {
    Table,
    Series,
}
