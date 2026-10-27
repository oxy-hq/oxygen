use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        create_runs(manager).await?;
        create_flips(manager).await?;
        let conn = manager.get_connection();
        // One run per experiment name per workspace, ever: a registered
        // experiment is immutable, and a new design is a new file.
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS uq_experiment_runs_workspace_name \
             ON experiment_runs (workspace_id, name)",
        )
        .await?;
        // The tick's due-run scan.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_experiment_runs_next_check \
             ON experiment_runs (next_check_at)",
        )
        .await?;
        // THE idempotency key: a repeated tick hits this and records nothing.
        // `due_on` is in it because a switchback flips one unit on many times.
        conn.execute_unprepared(
            "CREATE UNIQUE INDEX IF NOT EXISTS uq_experiment_flips_run_unit_state_due \
             ON experiment_flips (run_id, unit, target_state, due_on)",
        )
        .await?;
        // The sender's scan: a run's rows not yet stamped, by batch key.
        conn.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_experiment_flips_unsent \
             ON experiment_flips (run_id, wave, target_state, due_on) WHERE task_id IS NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(ExperimentFlips::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(
                Table::drop()
                    .table(ExperimentRuns::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

async fn create_runs(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    use ExperimentRuns as R;
    let ts = |c: R| {
        ColumnDef::new(c)
            .timestamp_with_time_zone()
            .not_null()
            .to_owned()
    };
    manager
        .create_table(
            Table::create()
                .table(R::Table)
                .if_not_exists()
                .col(ColumnDef::new(R::Id).uuid().not_null().primary_key())
                .col(ColumnDef::new(R::WorkspaceId).uuid().not_null())
                .col(ColumnDef::new(R::Name).text().not_null())
                .col(ColumnDef::new(R::RevisionId).uuid().not_null())
                .col(ts(R::RegisteredAt))
                .col(ColumnDef::new(R::State).text().not_null())
                .col(ts(R::NextCheckAt))
                .col(ColumnDef::new(R::ClosingSince).date().null())
                .col(ColumnDef::new(R::AbortReason).text().null())
                .col(ColumnDef::new(R::Config).json_binary().not_null())
                .col(ColumnDef::new(R::ControlPool).json_binary().null())
                .col(ColumnDef::new(R::GroupMembership).json_binary().null())
                .col(ColumnDef::new(R::Fleet).json_binary().null())
                .col(ColumnDef::new(R::GuardrailBreach).text().null())
                .col(ColumnDef::new(R::Observations).json_binary().null())
                .col(
                    ColumnDef::new(R::CheckEnqueuedAt)
                        .timestamp_with_time_zone()
                        .null(),
                )
                .col(ColumnDef::new(R::EstimateTaskId).uuid().null())
                .col(ts(R::CreatedAt))
                .col(ts(R::UpdatedAt))
                .to_owned(),
        )
        .await
}

async fn create_flips(manager: &SchemaManager<'_>) -> Result<(), DbErr> {
    use ExperimentFlips as F;
    manager
        .create_table(
            Table::create()
                .table(F::Table)
                .if_not_exists()
                .col(ColumnDef::new(F::Id).uuid().not_null().primary_key())
                .col(ColumnDef::new(F::RunId).uuid().not_null())
                .col(ColumnDef::new(F::Unit).text().not_null())
                .col(ColumnDef::new(F::TargetState).text().not_null())
                .col(ColumnDef::new(F::DueOn).date().not_null())
                .col(ColumnDef::new(F::Wave).integer().not_null())
                .col(ColumnDef::new(F::TaskId).uuid().null())
                .col(ColumnDef::new(F::AgenticRunId).uuid().null())
                .col(ColumnDef::new(F::LeverRevisionId).uuid().null())
                .col(ColumnDef::new(F::Status).text().not_null())
                .col(ColumnDef::new(F::ConfirmedOn).date().null())
                .col(ColumnDef::new(F::Reason).text().null())
                .col(
                    ColumnDef::new(F::CreatedAt)
                        .timestamp_with_time_zone()
                        .not_null(),
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk_experiment_flips_run")
                        .from(F::Table, F::RunId)
                        .to(ExperimentRuns::Table, ExperimentRuns::Id)
                        .on_delete(ForeignKeyAction::Cascade),
                )
                .to_owned(),
        )
        .await
}

#[derive(DeriveIden, Clone, Copy)]
enum ExperimentRuns {
    Table,
    Id,
    WorkspaceId,
    Name,
    RevisionId,
    RegisteredAt,
    State,
    NextCheckAt,
    ClosingSince,
    AbortReason,
    Config,
    ControlPool,
    GroupMembership,
    Fleet,
    GuardrailBreach,
    Observations,
    CheckEnqueuedAt,
    EstimateTaskId,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden, Clone, Copy)]
enum ExperimentFlips {
    Table,
    Id,
    RunId,
    Unit,
    TargetState,
    DueOn,
    Wave,
    TaskId,
    AgenticRunId,
    LeverRevisionId,
    Status,
    ConfirmedOn,
    Reason,
    CreatedAt,
}
