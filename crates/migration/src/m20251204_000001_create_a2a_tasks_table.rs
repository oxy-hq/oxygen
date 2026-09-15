use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create a2a_tasks table
        manager
            .create_table(
                Table::create()
                    .table(A2aTasks::Table)
                    .if_not_exists()
                    .col(uuid(A2aTasks::Id).primary_key())
                    .col(string(A2aTasks::AgentName))
                    .col(uuid_null(A2aTasks::ThreadId))
                    .col(uuid_null(A2aTasks::RunId))
                    .col(string_null(A2aTasks::ContextId))
                    .col(string(A2aTasks::State))
                    .col(json(A2aTasks::Metadata))
                    .col(
                        timestamp_with_time_zone(A2aTasks::CreatedAt)
                            .extra("DEFAULT CURRENT_TIMESTAMP".to_string()),
                    )
                    .col(
                        timestamp_with_time_zone(A2aTasks::UpdatedAt)
                            .extra("DEFAULT CURRENT_TIMESTAMP".to_string()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_a2a_tasks_thread_id")
                            .from(A2aTasks::Table, A2aTasks::ThreadId)
                            .to(Threads::Table, Threads::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::NoAction),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_a2a_tasks_run_id")
                            .from(A2aTasks::Table, A2aTasks::RunId)
                            .to(Runs::Table, Runs::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::NoAction),
                    )
                    .to_owned(),
            )
            .await?;

        // Create index on (agent_name, state) for efficient querying
        manager
            .create_index(
                Index::create()
                    .table(A2aTasks::Table)
                    .name("idx_a2a_tasks_agent_name_state")
                    .col(A2aTasks::AgentName)
                    .col(A2aTasks::State)
                    .to_owned(),
            )
            .await?;

        // Create index on context_id for context-based queries
        manager
            .create_index(
                Index::create()
                    .table(A2aTasks::Table)
                    .name("idx_a2a_tasks_context_id")
                    .col(A2aTasks::ContextId)
                    .to_owned(),
            )
            .await?;

        // Create index on thread_id for thread-based queries
        manager
            .create_index(
                Index::create()
                    .table(A2aTasks::Table)
                    .name("idx_a2a_tasks_thread_id")
                    .col(A2aTasks::ThreadId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(A2aTasks::Table).if_exists().to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum A2aTasks {
    Table,
    Id,
    AgentName,
    ThreadId,
    RunId,
    ContextId,
    State,
    Metadata,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Threads {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Runs {
    Table,
    Id,
}
