use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // One sealed row per run. A NEW table, so nothing an older binary reads
        // changes and a revert simply leaves it unused (revert-safe-migrations.md).
        manager
            .create_table(
                Table::create()
                    .table(ExperimentResults::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ExperimentResults::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(ExperimentResults::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    // Unique: the estimate job inserts `ON CONFLICT DO NOTHING`, so a
                    // retried or duplicated job cannot overwrite the registered result.
                    .col(
                        ColumnDef::new(ExperimentResults::RunId)
                            .uuid()
                            .not_null()
                            .unique_key(),
                    )
                    .col(
                        ColumnDef::new(ExperimentResults::Effect)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentResults::Ratio)
                            .json_binary()
                            .null(),
                    )
                    // Nullable: no frozen prior, no driver, or an abort. A column of a table created in this
                    // same migration — never an ALTER on a table an older binary reads.
                    .col(
                        ColumnDef::new(ExperimentResults::Prediction)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentResults::Compliance)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentResults::Report)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentResults::CoefficientPatch)
                            .text()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentResults::Aborted)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_experiment_results_run")
                            .from(ExperimentResults::Table, ExperimentResults::RunId)
                            .to(ExperimentRuns::Table, ExperimentRuns::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ExperimentResults::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ExperimentResults {
    Table,
    Id,
    CreatedAt,
    RunId,
    Effect,
    Ratio,
    Prediction,
    Compliance,
    Report,
    CoefficientPatch,
    Aborted,
}

/// PR 4's table, named only as the foreign-key target.
#[derive(DeriveIden)]
enum ExperimentRuns {
    Table,
    Id,
}
