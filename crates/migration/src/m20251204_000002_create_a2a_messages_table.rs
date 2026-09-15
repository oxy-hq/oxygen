use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create a2a_messages table for message history
        manager
            .create_table(
                Table::create()
                    .table(A2aMessages::Table)
                    .if_not_exists()
                    .col(uuid(A2aMessages::Id).primary_key())
                    .col(uuid_null(A2aMessages::TaskId))
                    .col(string_null(A2aMessages::ContextId))
                    .col(string(A2aMessages::AgentName))
                    .col(string(A2aMessages::Role))
                    .col(integer(A2aMessages::SequenceNumber))
                    .col(json(A2aMessages::Parts))
                    .col(json_null(A2aMessages::Metadata))
                    .col(
                        timestamp_with_time_zone(A2aMessages::CreatedAt)
                            .extra("DEFAULT CURRENT_TIMESTAMP".to_string()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_a2a_messages_task_id")
                            .from(A2aMessages::Table, A2aMessages::TaskId)
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
                    .table(A2aMessages::Table)
                    .name("idx_a2a_messages_task_sequence")
                    .col(A2aMessages::TaskId)
                    .col(A2aMessages::SequenceNumber)
                    .to_owned(),
            )
            .await?;

        // Create index on context_id for context-based retrieval
        manager
            .create_index(
                Index::create()
                    .table(A2aMessages::Table)
                    .name("idx_a2a_messages_context_id")
                    .col(A2aMessages::ContextId)
                    .to_owned(),
            )
            .await?;

        // Create index on (agent_name, task_id) for security filtering
        manager
            .create_index(
                Index::create()
                    .table(A2aMessages::Table)
                    .name("idx_a2a_messages_agent_task")
                    .col(A2aMessages::AgentName)
                    .col(A2aMessages::TaskId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(A2aMessages::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum A2aMessages {
    Table,
    Id,
    TaskId,
    ContextId,
    AgentName,
    Role,
    SequenceNumber,
    Parts,
    Metadata,
    CreatedAt,
}

#[derive(DeriveIden)]
enum A2aTasks {
    Table,
    Id,
}
