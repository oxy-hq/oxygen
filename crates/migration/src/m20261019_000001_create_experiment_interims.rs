use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // The latest interim reading of a run in progress: one row per run, overwritten by
        // each reading. Never the sealed result (`experiment_results`). A NEW table, so an
        // older binary never reads it and a revert leaves it unused (revert-safe-migrations.md).
        manager
            .create_table(
                Table::create()
                    .table(ExperimentInterims::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ExperimentInterims::RunId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    // The job in flight, if any: the enqueue's compare-and-swap reads it.
                    .col(ColumnDef::new(ExperimentInterims::TaskId).uuid().null())
                    .col(
                        ColumnDef::new(ExperimentInterims::RequestedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentInterims::ComputedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(ExperimentInterims::Through).date().null())
                    .col(
                        ColumnDef::new(ExperimentInterims::Report)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentInterims::Effect)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentInterims::Compliance)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(ExperimentInterims::Series)
                            .json_binary()
                            .null(),
                    )
                    // Why the last attempt produced no reading; cleared by the next one that does.
                    .col(ColumnDef::new(ExperimentInterims::Error).text().null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_experiment_interims_run")
                            .from(ExperimentInterims::Table, ExperimentInterims::RunId)
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
                    .table(ExperimentInterims::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ExperimentInterims {
    Table,
    RunId,
    TaskId,
    RequestedAt,
    ComputedAt,
    Through,
    Report,
    Effect,
    Compliance,
    Series,
    Error,
}

/// PR 4's table, named only as the foreign-key target.
#[derive(DeriveIden)]
enum ExperimentRuns {
    Table,
    Id,
}
