//! Drop the retired `runs` table.
//!
//! The pre-agentic executor's per-run store — `entity::runs` plus
//! `core/adapters/runs` (`RunsManager` / `Broadcaster`) — was retired in the
//! old-executor cleanup. The last writer (`RunsManager::new_run`) lost its caller
//! in Phase 4a/4b and the completion/block writer `upsert_run` was deleted in
//! #3123, leaving the table write-dead; its readers (`api/message.rs`,
//! `api/run.rs`, `api/task.rs`) and the whole FE agentic-thread stack were removed
//! in this change. The live agentic chat reloads through the agentic run store
//! (`agentic_runs`, `GET /analytics/threads/{id}/runs`), not this table.
//!
//! One incoming FK must go first: the orphaned `a2a_tasks` table (no entity, no
//! code references it) carries `fk_a2a_tasks_run_id` (ON DELETE SET NULL) →
//! `runs.id`. `up` drops that constraint, then the table; `down` recreates the
//! table and re-adds the constraint. The table is retired, so `down` exists only
//! to keep the migration reversible.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Drop the orphaned `a2a_tasks -> runs` FK so the table can be dropped.
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "a2a_tasks" DROP CONSTRAINT IF EXISTS "fk_a2a_tasks_run_id""#,
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Runs::Table).if_exists().to_owned())
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Faithful reverse of the final `runs` schema: the create
        // (m20250727_150336) plus the user_id / variables+output / project+branch_id
        // / root_replay_ref-as-text alters, its two indexes, its three outgoing FKs,
        // and the `a2a_tasks` incoming FK.
        manager
            .create_table(
                Table::create()
                    .table(Runs::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Runs::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Runs::SourceId).string().not_null())
                    .col(ColumnDef::new(Runs::RunIndex).integer().null())
                    .col(ColumnDef::new(Runs::RootSourceId).string().null())
                    .col(ColumnDef::new(Runs::RootRunIndex).integer().null())
                    .col(ColumnDef::new(Runs::RootReplayRef).text().null())
                    .col(ColumnDef::new(Runs::Metadata).json().null())
                    .col(ColumnDef::new(Runs::Children).json().null())
                    .col(ColumnDef::new(Runs::Blocks).json().null())
                    .col(ColumnDef::new(Runs::Variables).json().null())
                    .col(ColumnDef::new(Runs::Output).json().null())
                    .col(ColumnDef::new(Runs::Error).string().null())
                    .col(ColumnDef::new(Runs::ProjectId).uuid().not_null())
                    .col(ColumnDef::new(Runs::BranchId).uuid().not_null())
                    .col(ColumnDef::new(Runs::LookupId).uuid().null())
                    .col(ColumnDef::new(Runs::UserId).uuid().null())
                    .col(
                        ColumnDef::new(Runs::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Runs::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_runs_project_id")
                            .from(Runs::Table, Runs::ProjectId)
                            .to(Workspaces::Table, Workspaces::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_runs_lookup_id")
                            .from(Runs::Table, Runs::LookupId)
                            .to(Messages::Table, Messages::Id)
                            .on_delete(ForeignKeyAction::SetNull),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_runs_user_id")
                            .from(Runs::Table, Runs::UserId)
                            .to(Users::Table, Users::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_unique_source_runindex")
                    .table(Runs::Table)
                    .col(Runs::ProjectId)
                    .col(Runs::BranchId)
                    .col(Runs::SourceId)
                    .col(Runs::RunIndex)
                    .cond_where(Expr::col(Runs::RunIndex).is_not_null())
                    .unique()
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .name("idx_runs_user_id")
                    .table(Runs::Table)
                    .col(Runs::UserId)
                    .to_owned(),
            )
            .await?;
        // Re-add the `a2a_tasks -> runs` incoming FK.
        manager
            .get_connection()
            .execute_unprepared(
                r#"ALTER TABLE "a2a_tasks" ADD CONSTRAINT "fk_a2a_tasks_run_id" FOREIGN KEY ("run_id") REFERENCES "runs" ("id") ON DELETE SET NULL"#,
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Runs {
    Table,
    Id,
    SourceId,
    RunIndex,
    RootSourceId,
    RootRunIndex,
    RootReplayRef,
    Metadata,
    Children,
    Blocks,
    Variables,
    Output,
    Error,
    ProjectId,
    BranchId,
    LookupId,
    UserId,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Workspaces {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Messages {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum Users {
    Table,
    Id,
}
