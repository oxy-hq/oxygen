use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create a2a_task_status table for status history
        manager
            .create_table(
                Table::create()
                    .table(A2aTaskStatus::Table)
                    .if_not_exists()
                    .col(uuid(A2aTaskStatus::Id).primary_key())
                    .col(uuid(A2aTaskStatus::TaskId))
                    .col(string(A2aTaskStatus::AgentName))
                    .col(string(A2aTaskStatus::State))
                    .col(uuid_null(A2aTaskStatus::MessageId))
                    .col(json_null(A2aTaskStatus::Metadata))
                    .col(
                        timestamp_with_time_zone(A2aTaskStatus::CreatedAt)
                            .extra("DEFAULT CURRENT_TIMESTAMP".to_string()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_a2a_task_status_task_id")
                            .from(A2aTaskStatus::Table, A2aTaskStatus::TaskId)
                            .to(A2aTasks::Table, A2aTasks::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::NoAction),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_a2a_task_status_message_id")
                            .from(A2aTaskStatus::Table, A2aTaskStatus::MessageId)
                            .to(A2aMessages::Table, A2aMessages::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::NoAction),
                    )
                    .to_owned(),
            )
            .await?;

        // Create composite index on (task_id, created_at DESC) for chronological retrieval
        manager
            .create_index(
                Index::create()
                    .table(A2aTaskStatus::Table)
                    .name("idx_a2a_task_status_task_created")
                    .col(A2aTaskStatus::TaskId)
                    .col(A2aTaskStatus::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // Create index on (agent_name, task_id) for security filtering
        manager
            .create_index(
                Index::create()
                    .table(A2aTaskStatus::Table)
                    .name("idx_a2a_task_status_agent_task")
                    .col(A2aTaskStatus::AgentName)
                    .col(A2aTaskStatus::TaskId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(A2aTaskStatus::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum A2aTaskStatus {
    Table,
    Id,
    TaskId,
    AgentName,
    State,
    MessageId,
    Metadata,
    CreatedAt,
}

#[derive(DeriveIden)]
enum A2aTasks {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum A2aMessages {
    Table,
    Id,
}
