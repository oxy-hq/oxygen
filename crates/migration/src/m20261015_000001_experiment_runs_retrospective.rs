use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // A retrospective run: created `closing` from a design whose dates are all past,
        // never checked, no lever moved, reported as not pre-registered. Additive with a
        // default, so every existing run reads `false` and an older binary never selects
        // it (revert-safe-migrations.md).
        manager
            .alter_table(
                Table::alter()
                    .table(ExperimentRuns::Table)
                    .add_column_if_not_exists(
                        ColumnDef::new(ExperimentRuns::Retrospective)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ExperimentRuns::Table)
                    .drop_column(ExperimentRuns::Retrospective)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ExperimentRuns {
    Table,
    Retrospective,
}
