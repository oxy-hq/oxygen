use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create a2a_artifacts table for artifact storage
        manager
            .create_table(
                Table::create()
                    .table(A2aArtifacts::Table)
                    .if_not_exists()
                    .col(uuid(A2aArtifacts::Id).primary_key())
                    .col(uuid(A2aArtifacts::TaskId))
                    .col(string(A2aArtifacts::AgentName))
                    .col(integer(A2aArtifacts::SequenceNumber))
                    .col(text_null(A2aArtifacts::Description))
                    .col(json(A2aArtifacts::Parts))
                    .col(string_null(A2aArtifacts::StorageLocation))
                    .col(big_integer_null(A2aArtifacts::SizeBytes))
                    .col(json_null(A2aArtifacts::Metadata))
                    .col(
                        timestamp_with_time_zone(A2aArtifacts::CreatedAt)
                            .extra("DEFAULT CURRENT_TIMESTAMP".to_string()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_a2a_artifacts_task_id")
                            .from(A2aArtifacts::Table, A2aArtifacts::TaskId)
                            .to(A2aTasks::Table, A2aTasks::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::NoAction),
                    )
                    .to_owned(),
            )
            .await?;

        // Create composite index on (task_id, sequence_number) for ordered retrieval
        manager
            .create_index(
                Index::create()
                    .table(A2aArtifacts::Table)
                    .name("idx_a2a_artifacts_task_sequence")
                    .col(A2aArtifacts::TaskId)
                    .col(A2aArtifacts::SequenceNumber)
                    .to_owned(),
            )
            .await?;

        // Create index on (agent_name, task_id) for security filtering
        manager
            .create_index(
                Index::create()
                    .table(A2aArtifacts::Table)
                    .name("idx_a2a_artifacts_agent_task")
                    .col(A2aArtifacts::AgentName)
                    .col(A2aArtifacts::TaskId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(A2aArtifacts::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum A2aArtifacts {
    Table,
    Id,
    TaskId,
    AgentName,
    SequenceNumber,
    Description,
    Parts,
    StorageLocation,
    SizeBytes,
    Metadata,
    CreatedAt,
}

#[derive(DeriveIden)]
enum A2aTasks {
    Table,
    Id,
}
