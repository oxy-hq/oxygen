//! SeaORM migrations for the orchestrator runtime.
//!
//! Uses a **separate tracking table** (`seaql_migrations_orchestrator`) so this
//! migrator is fully independent of the central `crates/migration` migrator.

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use sea_orm_migration::prelude::*;

pub struct RuntimeMigrator;

#[async_trait::async_trait]
impl MigratorTrait for RuntimeMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(CreateAgenticTables),
            Box::new(RenameLegacySingularTables),
            Box::new(AddExtensibilityColumns),
            Box::new(DropLegacyDomainColumns),
            Box::new(AddTaskTreeColumns),
            Box::new(CreateTaskOutcomesTable),
            Box::new(AddAttemptColumn),
            Box::new(AddEventAttemptColumn),
            Box::new(CreateTaskQueueTable),
            Box::new(RationalizeStatusModel),
            Box::new(AddTaskQueueNotifyTrigger),
            Box::new(AddScopeOwnedAndDriverLease),
            Box::new(CreateSchedulesTable),
            Box::new(AddScheduleLastError),
            Box::new(AddRunCancelRequested),
            Box::new(AddScheduleWorkspaceId),
            Box::new(AddRunWorkspaceId),
            Box::new(AddScheduleMissedRuns),
            Box::new(AddRunScheduleId),
            Box::new(AddScheduleQuestion),
            Box::new(AddTaskQueueAvailableAt),
            Box::new(AddTaskQueueFirstDeferredAt),
            Box::new(AddPendingGlobalRunsIndex),
            Box::new(AddCompileTaskBackoffIndex),
        ]
    }

    fn migration_table_name() -> sea_orm::DynIden {
        Alias::new("seaql_migrations_orchestrator").into_iden()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

async fn table_exists(manager: &SchemaManager<'_>, table: &str) -> Result<bool, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM information_schema.tables WHERE table_name = $1 LIMIT 1",
        [table.into()],
    );
    Ok(manager
        .get_connection()
        .query_one_raw(stmt)
        .await?
        .is_some())
}

async fn column_exists(
    manager: &SchemaManager<'_>,
    table: &str,
    column: &str,
) -> Result<bool, DbErr> {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT 1 FROM information_schema.columns WHERE table_name = $1 AND column_name = $2 LIMIT 1",
        [table.into(), column.into()],
    );
    Ok(manager
        .get_connection()
        .query_one_raw(stmt)
        .await?
        .is_some())
}

async fn rename_table_if_needed(
    manager: &SchemaManager<'_>,
    from: &str,
    to: &str,
) -> Result<(), DbErr> {
    if table_exists(manager, to).await? || !table_exists(manager, from).await? {
        return Ok(());
    }
    manager
        .rename_table(
            Table::rename()
                .table(Alias::new(from), Alias::new(to))
                .to_owned(),
        )
        .await
}

// ── Iden enums ───────────────────────────────────────────────────────────────

#[derive(Iden)]
enum AgenticRun {
    #[iden = "agentic_runs"]
    Table,
    Id,
    AgentId,
    Question,
    Status,
    Answer,
    ErrorMessage,
    CreatedAt,
    UpdatedAt,
    ParentRunId,
    TaskStatus,
    TaskMetadata,
}

#[derive(Iden)]
enum AgenticRunEvent {
    #[iden = "agentic_run_events"]
    Table,
    Id,
    RunId,
    Seq,
    EventType,
    Payload,
    CreatedAt,
}

#[derive(Iden)]
enum AgenticRunSuspension {
    #[iden = "agentic_run_suspensions"]
    Table,
    RunId,
    Prompt,
    Suggestions,
    ResumeData,
    CreatedAt,
}

// ── Migration 1: Create tables ──────────────────────────────────────────────

struct CreateAgenticTables;

impl MigrationName for CreateAgenticTables {
    fn name(&self) -> &str {
        "m20260317_000001_create_agentic_tables"
    }
}

/// The `agentic_runs` create-table statement.
///
/// **Twin: `agentic_runs_table()` in `crates/migration`.** Both crates create
/// this table with `if_not_exists`, so a column added here and not there (or
/// vice versa) does not fail — whichever migrator runs first wins and the other
/// no-ops, and the divergence shows up as a missing column at runtime. Edit
/// both. They deliberately render *different* identifiers: plural here (pinned
/// with `#[iden]`), singular there until `m20260317_000002` renames it.
///
/// Pins the **create** shape only; the `parent_run_id` / `task_status` /
/// `task_metadata` columns added by later migrations in this file are guarded
/// by `column_exists` and are not part of this statement.
fn agentic_runs_table() -> TableCreateStatement {
    Table::create()
        .table(AgenticRun::Table)
        .if_not_exists()
        .col(
            ColumnDef::new(AgenticRun::Id)
                .string()
                .not_null()
                .primary_key(),
        )
        .col(ColumnDef::new(AgenticRun::AgentId).string().not_null())
        .col(ColumnDef::new(AgenticRun::Question).text().not_null())
        .col(
            ColumnDef::new(AgenticRun::Status)
                .string()
                .not_null()
                .default("running"),
        )
        .col(ColumnDef::new(AgenticRun::Answer).text().null())
        .col(ColumnDef::new(AgenticRun::ErrorMessage).text().null())
        .col(
            ColumnDef::new(AgenticRun::CreatedAt)
                .timestamp_with_time_zone()
                .not_null(),
        )
        .col(
            ColumnDef::new(AgenticRun::UpdatedAt)
                .timestamp_with_time_zone()
                .not_null(),
        )
        .to_owned()
}

/// The `agentic_run_suspensions` create-table statement.
///
/// **Twin: `agentic_run_suspensions_table()` in `crates/migration`** — same
/// `if_not_exists` hazard and same deliberate identifier difference as
/// `agentic_runs_table()` above. Edit both.
fn agentic_run_suspensions_table() -> TableCreateStatement {
    Table::create()
        .table(AgenticRunSuspension::Table)
        .if_not_exists()
        .col(
            ColumnDef::new(AgenticRunSuspension::RunId)
                .string()
                .not_null()
                .primary_key(),
        )
        .col(
            ColumnDef::new(AgenticRunSuspension::Prompt)
                .text()
                .not_null(),
        )
        .col(
            ColumnDef::new(AgenticRunSuspension::Suggestions)
                .json_binary()
                .not_null(),
        )
        .col(
            ColumnDef::new(AgenticRunSuspension::ResumeData)
                .json_binary()
                .not_null(),
        )
        .col(
            ColumnDef::new(AgenticRunSuspension::CreatedAt)
                .timestamp_with_time_zone()
                .not_null(),
        )
        .foreign_key(
            ForeignKey::create()
                .from(AgenticRunSuspension::Table, AgenticRunSuspension::RunId)
                .to(AgenticRun::Table, AgenticRun::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

/// The unique index on `agentic_run_events (run_id, seq)`.
///
/// **Twin: `agentic_run_events_index()` in `crates/migration`**, and a sharper
/// hazard than the tables: the index *name* is identical on both sides, so
/// `if_not_exists` no-ops by name. Change the columns or drop `.unique()` on
/// one side and the other silently does nothing — no error, just an index that
/// isn't the one you wrote.
fn agentic_run_events_index() -> IndexCreateStatement {
    Index::create()
        .if_not_exists()
        .name("idx_agentic_run_events_run_id_seq")
        .table(AgenticRunEvent::Table)
        .col(AgenticRunEvent::RunId)
        .col(AgenticRunEvent::Seq)
        .unique()
        .to_owned()
}

/// The `agentic_run_events` create-table statement.
///
/// **Twin: `agentic_run_events_table()` in `crates/migration`** — same
/// `if_not_exists` hazard and same deliberate identifier difference as
/// `agentic_runs_table()` above. Edit both.
///
/// Extracted so the test at the bottom of this file asserts on the DDL this
/// migrator actually emits, rather than on a copy that can drift from it. The
/// whole rendering is pinned, not just the `bigserial` PK.
fn agentic_run_events_table() -> TableCreateStatement {
    Table::create()
        .table(AgenticRunEvent::Table)
        .if_not_exists()
        .col(
            // Spelled `bigserial` explicitly — see the twin migration in
            // `crates/migration`. `.auto_increment()` renders identity columns
            // on SeaORM 2.0 without the non-default `postgres-use-serial-pk`
            // feature, and this table already exists as `bigserial` in every
            // deployed database.
            ColumnDef::new(AgenticRunEvent::Id)
                .custom(Alias::new("bigserial"))
                .not_null()
                .primary_key(),
        )
        .col(ColumnDef::new(AgenticRunEvent::RunId).string().not_null())
        .col(
            ColumnDef::new(AgenticRunEvent::Seq)
                .big_integer()
                .not_null(),
        )
        .col(
            ColumnDef::new(AgenticRunEvent::EventType)
                .string()
                .not_null(),
        )
        .col(
            ColumnDef::new(AgenticRunEvent::Payload)
                .json_binary()
                .not_null(),
        )
        .col(
            ColumnDef::new(AgenticRunEvent::CreatedAt)
                .timestamp_with_time_zone()
                .not_null(),
        )
        .foreign_key(
            ForeignKey::create()
                .from(AgenticRunEvent::Table, AgenticRunEvent::RunId)
                .to(AgenticRun::Table, AgenticRun::Id)
                .on_delete(ForeignKeyAction::Cascade),
        )
        .to_owned()
}

#[async_trait::async_trait]
impl MigrationTrait for CreateAgenticTables {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.create_table(agentic_runs_table()).await?;

        manager.create_table(agentic_run_events_table()).await?;

        manager.create_index(agentic_run_events_index()).await?;

        manager
            .create_table(agentic_run_suspensions_table())
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AgenticRunSuspension::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AgenticRunEvent::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(AgenticRun::Table).to_owned())
            .await?;
        Ok(())
    }
}

// ── Migration 2: Rename legacy singular table names ─────────────────────────

struct RenameLegacySingularTables;

impl MigrationName for RenameLegacySingularTables {
    fn name(&self) -> &str {
        "m20260317_000002_rename_legacy_singular_tables"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for RenameLegacySingularTables {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rename_table_if_needed(manager, "agentic_run_suspension", "agentic_run_suspensions")
            .await?;
        rename_table_if_needed(manager, "agentic_run_event", "agentic_run_events").await?;
        rename_table_if_needed(manager, "agentic_run", "agentic_runs").await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        rename_table_if_needed(manager, "agentic_run_suspensions", "agentic_run_suspension")
            .await?;
        rename_table_if_needed(manager, "agentic_run_events", "agentic_run_event").await?;
        rename_table_if_needed(manager, "agentic_runs", "agentic_run").await?;
        Ok(())
    }
}

// ── Migration 3: Add extensibility columns ──────────────────────────────────

struct AddExtensibilityColumns;

impl MigrationName for AddExtensibilityColumns {
    fn name(&self) -> &str {
        "m20260407_000001_add_extensibility_columns"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddExtensibilityColumns {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Add source_type column if not exists.
        if !column_exists(manager, "agentic_runs", "source_type").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(ColumnDef::new(Alias::new("source_type")).string().null())
                        .to_owned(),
                )
                .await?;
        }

        // Add metadata column if not exists.
        if !column_exists(manager, "agentic_runs", "metadata").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(ColumnDef::new(Alias::new("metadata")).json_binary().null())
                        .to_owned(),
                )
                .await?;
        }

        // Backfill source_type from agent_id for existing rows.
        db.execute_unprepared(
            "UPDATE agentic_runs SET source_type = CASE \
                 WHEN agent_id = '__builder__' THEN 'builder' \
                 ELSE 'analytics' \
             END \
             WHERE source_type IS NULL",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_runs", "metadata").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .drop_column(Alias::new("metadata"))
                        .to_owned(),
                )
                .await?;
        }
        if column_exists(manager, "agentic_runs", "source_type").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .drop_column(Alias::new("source_type"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

// ── Migration 4: Drop legacy domain-specific columns ────────────────────────

struct DropLegacyDomainColumns;

impl MigrationName for DropLegacyDomainColumns {
    fn name(&self) -> &str {
        "m20260408_000001_drop_legacy_domain_columns"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for DropLegacyDomainColumns {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // These columns have been migrated to analytics_run_extensions.
        // Use IF EXISTS for idempotency.
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE agentic_runs DROP COLUMN IF EXISTS agent_id; \
             ALTER TABLE agentic_runs DROP COLUMN IF EXISTS spec_hint; \
             ALTER TABLE agentic_runs DROP COLUMN IF EXISTS thinking_mode;",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Re-add the columns if needed for rollback.
        let db = manager.get_connection();
        if !column_exists(manager, "agentic_runs", "agent_id").await? {
            db.execute_unprepared(
                "ALTER TABLE agentic_runs ADD COLUMN agent_id TEXT NOT NULL DEFAULT ''",
            )
            .await?;
        }
        if !column_exists(manager, "agentic_runs", "spec_hint").await? {
            db.execute_unprepared("ALTER TABLE agentic_runs ADD COLUMN spec_hint JSONB")
                .await?;
        }
        if !column_exists(manager, "agentic_runs", "thinking_mode").await? {
            db.execute_unprepared("ALTER TABLE agentic_runs ADD COLUMN thinking_mode TEXT")
                .await?;
        }
        Ok(())
    }
}

// ── Migration 5: Add task tree columns ─────────────────────────────────────

struct AddTaskTreeColumns;

impl MigrationName for AddTaskTreeColumns {
    fn name(&self) -> &str {
        "m20260412_000001_add_task_tree_columns"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddTaskTreeColumns {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // parent_run_id: self-referential FK for task tree.
        if !column_exists(manager, "agentic_runs", "parent_run_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(ColumnDef::new(AgenticRun::ParentRunId).string().null())
                        .add_foreign_key(
                            TableForeignKey::new()
                                .name("fk_agentic_runs_parent_run_id")
                                .from_tbl(AgenticRun::Table)
                                .from_col(AgenticRun::ParentRunId)
                                .to_tbl(AgenticRun::Table)
                                .to_col(AgenticRun::Id)
                                .on_delete(ForeignKeyAction::Cascade),
                        )
                        .to_owned(),
                )
                .await?;

            manager
                .create_index(
                    Index::create()
                        .name("idx_agentic_runs_parent_run_id")
                        .table(AgenticRun::Table)
                        .col(AgenticRun::ParentRunId)
                        .if_not_exists()
                        .to_owned(),
                )
                .await?;
        }

        // task_status: coordinator's internal status (running, suspended_human,
        // waiting_on_child, done, failed). Distinct from user-facing `status`.
        if !column_exists(manager, "agentic_runs", "task_status").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(ColumnDef::new(AgenticRun::TaskStatus).string().null())
                        .to_owned(),
                )
                .await?;
        }

        // task_metadata: extensible JSONB for coordinator state (child_task_ids, etc.).
        if !column_exists(manager, "agentic_runs", "task_metadata").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(
                            ColumnDef::new(AgenticRun::TaskMetadata)
                                .json_binary()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE agentic_runs \
             DROP COLUMN IF EXISTS task_metadata, \
             DROP COLUMN IF EXISTS task_status, \
             DROP CONSTRAINT IF EXISTS fk_agentic_runs_parent_run_id, \
             DROP COLUMN IF EXISTS parent_run_id;",
        )
        .await?;
        Ok(())
    }
}

// ── Migration 6: Create task outcomes table ────────────────────────────────
//
// Single source of truth for child→parent result handoff. Written atomically
// before updating parent metadata, closing the crash-consistency window.

#[derive(Iden)]
enum AgenticTaskOutcome {
    #[iden = "agentic_task_outcomes"]
    Table,
    ChildId,
    ParentId,
    Status,
    Answer,
    CreatedAt,
}

struct CreateTaskOutcomesTable;

impl MigrationName for CreateTaskOutcomesTable {
    fn name(&self) -> &str {
        "m20260413_000001_create_task_outcomes_table"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateTaskOutcomesTable {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if table_exists(manager, "agentic_task_outcomes").await? {
            return Ok(());
        }

        manager
            .create_table(
                Table::create()
                    .table(AgenticTaskOutcome::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AgenticTaskOutcome::ChildId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskOutcome::ParentId)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskOutcome::Status)
                            .string()
                            .not_null(),
                    )
                    .col(ColumnDef::new(AgenticTaskOutcome::Answer).text().null())
                    .col(
                        ColumnDef::new(AgenticTaskOutcome::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AgenticTaskOutcome::Table, AgenticTaskOutcome::ChildId)
                            .to(AgenticRun::Table, AgenticRun::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_agentic_task_outcomes_parent_id")
                    .table(AgenticTaskOutcome::Table)
                    .col(AgenticTaskOutcome::ParentId)
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(AgenticTaskOutcome::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 7: Add attempt column ───────────────────────────────────────
//
// Tracks recovery attempts. 0 = original run, incremented on each recovery.
// Allows navigating between attempts in the UI.

struct AddAttemptColumn;

impl MigrationName for AddAttemptColumn {
    fn name(&self) -> &str {
        "m20260413_000002_add_attempt_column"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddAttemptColumn {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_runs", "attempt").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("attempt"))
                                .integer()
                                .not_null()
                                .default(0),
                        )
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_runs", "attempt").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .drop_column(Alias::new("attempt"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

// ── Migration 8: Add attempt column to events ─────────────────────────────
//
// Tags each event with its recovery attempt number so the frontend can
// distinguish events from different attempts.

struct AddEventAttemptColumn;

impl MigrationName for AddEventAttemptColumn {
    fn name(&self) -> &str {
        "m20260415_000001_add_event_attempt_column"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddEventAttemptColumn {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_run_events", "attempt").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRunEvent::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("attempt"))
                                .integer()
                                .not_null()
                                .default(0),
                        )
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_run_events", "attempt").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRunEvent::Table)
                        .drop_column(Alias::new("attempt"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

// ── Migration 9: Create task queue table ────────────────────────────────────
//
// Durable task queue inspired by Temporal. Assignments are persisted before
// dispatch; workers poll the table. Survives process crashes.

#[derive(Iden)]
enum AgenticTaskQueue {
    #[iden = "agentic_task_queue"]
    Table,
    TaskId,
    RunId,
    ParentTaskId,
    QueueStatus,
    Spec,
    Policy,
    WorkerId,
    LastHeartbeat,
    ClaimedAt,
    VisibilityTimeoutSecs,
    ClaimCount,
    MaxClaims,
    CreatedAt,
    UpdatedAt,
}

struct CreateTaskQueueTable;

impl MigrationName for CreateTaskQueueTable {
    fn name(&self) -> &str {
        "m20260415_000002_create_task_queue_table"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateTaskQueueTable {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if table_exists(manager, "agentic_task_queue").await? {
            return Ok(());
        }

        manager
            .create_table(
                Table::create()
                    .table(AgenticTaskQueue::Table)
                    .col(
                        ColumnDef::new(AgenticTaskQueue::TaskId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AgenticTaskQueue::RunId).string().not_null())
                    .col(
                        ColumnDef::new(AgenticTaskQueue::ParentTaskId)
                            .string()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::QueueStatus)
                            .string()
                            .not_null()
                            .default("queued"),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::Spec)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::Policy)
                            .json_binary()
                            .null(),
                    )
                    .col(ColumnDef::new(AgenticTaskQueue::WorkerId).string().null())
                    .col(
                        ColumnDef::new(AgenticTaskQueue::LastHeartbeat)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::ClaimedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::VisibilityTimeoutSecs)
                            .integer()
                            .not_null()
                            .default(60),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::ClaimCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::MaxClaims)
                            .integer()
                            .not_null()
                            .default(3),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgenticTaskQueue::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AgenticTaskQueue::Table, AgenticTaskQueue::RunId)
                            .to(AgenticRun::Table, AgenticRun::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Partial index for polling: only queued tasks, ordered by created_at.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_task_queue_poll \
                 ON agentic_task_queue (created_at) \
                 WHERE queue_status = 'queued'",
            )
            .await?;

        // Partial index for reaper: only claimed tasks, ordered by last_heartbeat.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_task_queue_reap \
                 ON agentic_task_queue (last_heartbeat) \
                 WHERE queue_status = 'claimed'",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(AgenticTaskQueue::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 10: Rationalize status model ──────────────────────────────────
//
// Drop the redundant `status` column (now derived from `task_status` at the API
// layer). Add `recovery_requested_at` column to replace `needs_resume`/`shutdown`
// task_status values. Rename task_status values to Temporal-inspired names.

struct RationalizeStatusModel;

impl MigrationName for RationalizeStatusModel {
    fn name(&self) -> &str {
        "m20260415_000003_rationalize_status_model"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for RationalizeStatusModel {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Drop the redundant `status` column.
        if column_exists(manager, "agentic_runs", "status").await? {
            db.execute_unprepared("ALTER TABLE agentic_runs DROP COLUMN status")
                .await?;
        }

        if !column_exists(manager, "agentic_runs", "recovery_requested_at").await? {
            db.execute_unprepared(
                "ALTER TABLE agentic_runs ADD COLUMN recovery_requested_at TIMESTAMPTZ",
            )
            .await?;
        }

        // Ensure thread_id column exists (may have been added by central migrator
        // or may be missing in test databases that only run runtime migrations).
        if !column_exists(manager, "agentic_runs", "thread_id").await? {
            db.execute_unprepared("ALTER TABLE agentic_runs ADD COLUMN thread_id UUID")
                .await?;
        }

        // Rename task_status values to new names (idempotent).
        db.execute_unprepared(
            "UPDATE agentic_runs SET task_status = 'awaiting_input' WHERE task_status = 'suspended_human'; \
             UPDATE agentic_runs SET task_status = 'delegating' WHERE task_status IN ('waiting_on_child', 'waiting_on_children'); \
             UPDATE agentic_runs SET recovery_requested_at = updated_at WHERE task_status IN ('needs_resume', 'shutdown'); \
             UPDATE agentic_runs SET task_status = 'running' WHERE task_status IN ('needs_resume', 'shutdown');",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        if !column_exists(manager, "agentic_runs", "status").await? {
            db.execute_unprepared(
                "ALTER TABLE agentic_runs ADD COLUMN status TEXT NOT NULL DEFAULT 'running'",
            )
            .await?;
        }

        if column_exists(manager, "agentic_runs", "recovery_requested_at").await? {
            db.execute_unprepared("ALTER TABLE agentic_runs DROP COLUMN recovery_requested_at")
                .await?;
        }

        // Revert task_status renames.
        db.execute_unprepared(
            "UPDATE agentic_runs SET task_status = 'suspended_human' WHERE task_status = 'awaiting_input'; \
             UPDATE agentic_runs SET task_status = 'waiting_on_children' WHERE task_status = 'delegating';",
        )
        .await?;

        Ok(())
    }
}

// ── Migration 11: Task-queue NOTIFY trigger ─────────────────────────────────
//
// Fires `pg_notify('oxy_task_enqueued', '')` on every row that lands in
// `queue_status = 'queued'` — both fresh INSERT rows (enqueue_task) and
// UPDATE statements that transition into queued (requeue_task, reap_stale_tasks).
//
// Putting the NOTIFY in a trigger instead of every Rust call site means:
//   - Impossible to forget when adding a new enqueue path.
//   - Atomic with the row write — listeners always observe the row
//     because Postgres defers NOTIFY delivery until the issuing txn
//     commits.
//   - No extra round-trip per enqueue.
//
// Pairs with `agentic_runtime::router::PostgresTaskRouter`, which is
// what actually `LISTEN`s on `oxy_task_enqueued`.

struct AddTaskQueueNotifyTrigger;

impl MigrationName for AddTaskQueueNotifyTrigger {
    fn name(&self) -> &str {
        "m20260512_000001_add_task_queue_notify_trigger"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddTaskQueueNotifyTrigger {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // The trigger fires AFTER INSERT OR UPDATE. We could narrow it
        // with a WHEN clause on `OLD.queue_status IS DISTINCT FROM
        // NEW.queue_status` to skip the per-row PL/pgSQL invocation on
        // pure metadata updates (heartbeat, claim_count), but PG WHEN
        // clauses don't have OLD on INSERT so we'd need two triggers.
        // Per-row PL/pgSQL overhead is ~microseconds; not worth the
        // added schema complexity for now.
        db.execute_unprepared(
            "CREATE OR REPLACE FUNCTION agentic_task_queue_notify_fn() \
             RETURNS TRIGGER AS $$ \
             BEGIN \
                 IF NEW.queue_status = 'queued' \
                    AND (TG_OP = 'INSERT' OR OLD.queue_status IS DISTINCT FROM 'queued') \
                 THEN \
                     PERFORM pg_notify('oxy_task_enqueued', ''); \
                 END IF; \
                 RETURN NEW; \
             END; \
             $$ LANGUAGE plpgsql;",
        )
        .await?;

        db.execute_unprepared(
            "DROP TRIGGER IF EXISTS agentic_task_queue_notify_trigger \
             ON agentic_task_queue;",
        )
        .await?;
        db.execute_unprepared(
            "CREATE TRIGGER agentic_task_queue_notify_trigger \
             AFTER INSERT OR UPDATE ON agentic_task_queue \
             FOR EACH ROW \
             EXECUTE FUNCTION agentic_task_queue_notify_fn();",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "DROP TRIGGER IF EXISTS agentic_task_queue_notify_trigger \
             ON agentic_task_queue;",
        )
        .await?;
        db.execute_unprepared("DROP FUNCTION IF EXISTS agentic_task_queue_notify_fn();")
            .await?;
        Ok(())
    }
}

// ── Migration 12: scope_owned + driver lease (cron scheduler Phase 1) ──────────
//
// Two-layer ownership model for the standalone/global driver:
//
//   Layer 1 — `agentic_task_queue.scope_owned`: set in the row's only INSERT.
//     A co-located scoped coordinator stamps `true`; scheduler-seeded / orphan
//     rows stay `false`. The global claim path filters `scope_owned = false`,
//     so it can never poach an interactive run's tasks. `reap_stale_tasks`
//     only flips `queue_status` and preserves this column, which is correct.
//
//   Layer 2 — `agentic_runs.driver_id` / `driver_heartbeat_at`: a CAS-acquired
//     lease that gates recovery *selection* so a periodic recovery loop cannot
//     double-drive an already-driven run.
//
// All three columns are nullable / defaulted so existing rows behave exactly
// as before (the feature is inert until the Phase 2 scheduler / global loop
// produces `scope_owned = false` work).

struct AddScopeOwnedAndDriverLease;

impl MigrationName for AddScopeOwnedAndDriverLease {
    fn name(&self) -> &str {
        "m20260519_000001_add_scope_owned_and_driver_lease"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddScopeOwnedAndDriverLease {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_task_queue", "scope_owned").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticTaskQueue::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("scope_owned"))
                                .boolean()
                                .not_null()
                                .default(false),
                        )
                        .to_owned(),
                )
                .await?;
        }

        if !column_exists(manager, "agentic_runs", "driver_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(ColumnDef::new(Alias::new("driver_id")).text().null())
                        .to_owned(),
                )
                .await?;
        }

        if !column_exists(manager, "agentic_runs", "driver_heartbeat_at").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("driver_heartbeat_at"))
                                .timestamp_with_time_zone()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_runs", "driver_heartbeat_at").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .drop_column(Alias::new("driver_heartbeat_at"))
                        .to_owned(),
                )
                .await?;
        }

        if column_exists(manager, "agentic_runs", "driver_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .drop_column(Alias::new("driver_id"))
                        .to_owned(),
                )
                .await?;
        }

        if column_exists(manager, "agentic_task_queue", "scope_owned").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticTaskQueue::Table)
                        .drop_column(Alias::new("scope_owned"))
                        .to_owned(),
                )
                .await?;
        }

        Ok(())
    }
}

// ── Migration 13: agentic_schedules (cron scheduler Phase 2) ──────────────────
//
// User-defined cron schedules for workflows / airway pipelines. The tick
// (gated behind OXY_INPROC_GLOBAL_WORKER) selects `enabled AND next_run_at
// <= now()`, CAS-advances `next_run_at`, and fires the seed fn with
// TaskScope::Global so the Phase-1 consumer drives it. `project_id` /
// `branch_id` are carried for future multi-workspace scoping but the first
// cut operates single-workspace.

#[derive(Iden)]
enum AgenticSchedule {
    #[iden = "agentic_schedules"]
    Table,
    Id,
    ProjectId,
    BranchId,
    Name,
    TargetKind,
    TargetRef,
    Variables,
    CronExpr,
    Timezone,
    Enabled,
    NextRunAt,
    LastFiredAt,
    LastRunId,
    CreatedAt,
    UpdatedAt,
}

struct CreateSchedulesTable;

impl MigrationName for CreateSchedulesTable {
    fn name(&self) -> &str {
        "m20260519_000002_create_agentic_schedules"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateSchedulesTable {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if table_exists(manager, "agentic_schedules").await? {
            return Ok(());
        }

        manager
            .create_table(
                Table::create()
                    .table(AgenticSchedule::Table)
                    .col(
                        ColumnDef::new(AgenticSchedule::Id)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(AgenticSchedule::ProjectId).string().null())
                    .col(ColumnDef::new(AgenticSchedule::BranchId).string().null())
                    .col(ColumnDef::new(AgenticSchedule::Name).string().not_null())
                    .col(
                        ColumnDef::new(AgenticSchedule::TargetKind)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgenticSchedule::TargetRef)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgenticSchedule::Variables)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AgenticSchedule::CronExpr)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgenticSchedule::Timezone)
                            .string()
                            .not_null()
                            .default("UTC"),
                    )
                    .col(
                        ColumnDef::new(AgenticSchedule::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(AgenticSchedule::NextRunAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgenticSchedule::LastFiredAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(ColumnDef::new(AgenticSchedule::LastRunId).string().null())
                    .col(
                        ColumnDef::new(AgenticSchedule::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AgenticSchedule::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Due-selection index: the tick filters `enabled AND next_run_at
        // <= now()` every cycle.
        manager
            .create_index(
                Index::create()
                    .name("idx_agentic_schedules_due")
                    .table(AgenticSchedule::Table)
                    .col(AgenticSchedule::Enabled)
                    .col(AgenticSchedule::NextRunAt)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AgenticSchedule::Table).to_owned())
            .await?;
        Ok(())
    }
}

// ── Migration 14: agentic_schedules.last_error (scheduler observability) ──────
//
// Records the most recent fire/seed failure (bad cron, missing target,
// seed error) so the UI can surface it instead of it living only in
// `tracing::warn`. Cleared on a successful fire.

struct AddScheduleLastError;

impl MigrationName for AddScheduleLastError {
    fn name(&self) -> &str {
        "m20260519_000003_add_schedule_last_error"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddScheduleLastError {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_schedules", "last_error").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticSchedule::Table)
                        .add_column(ColumnDef::new(Alias::new("last_error")).text().null())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_schedules", "last_error").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticSchedule::Table)
                        .drop_column(Alias::new("last_error"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

// ── Migration 15: agentic_runs.cancel_requested_at (§12 FU4a) ─────────────────
//
// DB-observable cancel signal. The in-memory watch channel only reaches a
// coordinator in the same process; a recovered / Global run driven by the
// periodic loop (or a future standalone worker) needs a durable,
// cross-process flag. HTTP cancel sets it; the driver's cancel forwarder
// polls it and tears down the subtree.

struct AddRunCancelRequested;

impl MigrationName for AddRunCancelRequested {
    fn name(&self) -> &str {
        "m20260519_000004_add_run_cancel_requested"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddRunCancelRequested {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_runs", "cancel_requested_at").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("cancel_requested_at"))
                                .timestamp_with_time_zone()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_runs", "cancel_requested_at").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .drop_column(Alias::new("cancel_requested_at"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

// ── Migration 16: agentic_schedules.workspace_id (§12 FU4b) ───────────────────
//
// Multi-tenant scoping. Plain UUID column, NO foreign key to
// `workspaces.id`: that table lives in the central migrator and per the
// agentic boundary rules (`crates/agentic/CLAUDE.md`) cross-domain
// references are loose — app-level lifecycle (workspace delete) handles
// cleanup. Backfilled to the nil UUID for any existing rows (the feature
// is inert until the multi-tenant tick lands, so this only affects dev
// DBs); new rows MUST set it via the CRUD handlers.

struct AddScheduleWorkspaceId;

impl MigrationName for AddScheduleWorkspaceId {
    fn name(&self) -> &str {
        "m20260520_000001_add_schedule_workspace_id"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddScheduleWorkspaceId {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_schedules", "workspace_id").await? {
            // Add NOT NULL with the nil UUID as a backfill default; future
            // inserts always pass a real value via the handlers.
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticSchedule::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("workspace_id"))
                                .uuid()
                                .not_null()
                                .default("00000000-0000-0000-0000-000000000000"),
                        )
                        .to_owned(),
                )
                .await?;
            // Index for the per-workspace due-selection query.
            manager
                .create_index(
                    Index::create()
                        .name("idx_agentic_schedules_workspace_due")
                        .table(AgenticSchedule::Table)
                        .col(Alias::new("workspace_id"))
                        .col(AgenticSchedule::Enabled)
                        .col(AgenticSchedule::NextRunAt)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_schedules", "workspace_id").await? {
            manager
                .drop_index(
                    Index::drop()
                        .name("idx_agentic_schedules_workspace_due")
                        .table(AgenticSchedule::Table)
                        .to_owned(),
                )
                .await
                .ok();
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticSchedule::Table)
                        .drop_column(Alias::new("workspace_id"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

// ── Migration 17: agentic_runs.workspace_id ──────────────────────────────────
//
// Closes the cloud-mode routing gap: without this column the periodic
// recovery loop and the latency worker can't tell which workspace's
// `PlatformContext` (project files, DB connectors, secrets) a row they
// pick up belongs to — they'd be forced to either iterate one row at a
// time across every workspace's context or run one worker per workspace
// (the §12.3 follow-up). With it, a single shared worker selects pending
// rows and routes each one to the right cached context via `ws_cache`.
//
// Same shape as `agentic_schedules.workspace_id` (FU4b): plain UUID, no
// foreign key to `workspaces.id` (cross-domain reference per
// `crates/agentic/CLAUDE.md`), NOT NULL with the nil UUID as backfill so
// the column lands without rewriting historical rows. `start_*_run`
// paths stamp it at insert from here on; the nil UUID functions as the
// implicit `LOCAL_WORKSPACE_ID` for the local serve mode.

struct AddRunWorkspaceId;

impl MigrationName for AddRunWorkspaceId {
    fn name(&self) -> &str {
        "m20260520_000002_add_run_workspace_id"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddRunWorkspaceId {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_runs", "workspace_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("workspace_id"))
                                .uuid()
                                .not_null()
                                .default("00000000-0000-0000-0000-000000000000"),
                        )
                        .to_owned(),
                )
                .await?;
            // Index for per-workspace selection by the recovery loop +
            // latency worker. Composite over `(workspace_id, task_status)`
            // because every workspace-scoped SELECT also narrows by
            // non-terminal task_status; keeps the planner happy without a
            // second, wider index.
            manager
                .create_index(
                    Index::create()
                        .name("idx_agentic_runs_workspace_status")
                        .table(AgenticRun::Table)
                        .col(Alias::new("workspace_id"))
                        .col(AgenticRun::TaskStatus)
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_runs", "workspace_id").await? {
            manager
                .drop_index(
                    Index::drop()
                        .name("idx_agentic_runs_workspace_status")
                        .table(AgenticRun::Table)
                        .to_owned(),
                )
                .await
                .ok();
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .drop_column(Alias::new("workspace_id"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

// ── Migration 18: agentic_schedules.missed_runs + last_missed_at ────────────
//
// Reports catch-up fires after the server was down: the run-once-then-resume
// policy in the tick (`scheduler.rs`) fires ONE catch-up and silently skips
// the rest. These two columns let the UI surface "N occurrences were missed"
// without changing that policy. Both are stamped by the tick when it detects
// > 0 occurrences in (prev_next_run_at, now] beyond the one being fired.

struct AddScheduleMissedRuns;

impl MigrationName for AddScheduleMissedRuns {
    fn name(&self) -> &str {
        "m20260520_000003_add_schedule_missed_runs"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddScheduleMissedRuns {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_schedules", "missed_runs").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticSchedule::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("missed_runs"))
                                .integer()
                                .not_null()
                                .default(0),
                        )
                        .to_owned(),
                )
                .await?;
        }
        if !column_exists(manager, "agentic_schedules", "last_missed_at").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticSchedule::Table)
                        .add_column(
                            ColumnDef::new(Alias::new("last_missed_at"))
                                .timestamp_with_time_zone()
                                .null(),
                        )
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for col in ["missed_runs", "last_missed_at"] {
            if column_exists(manager, "agentic_schedules", col).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(AgenticSchedule::Table)
                            .drop_column(Alias::new(col))
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}

// ── m20260524_000001 — link runs to the schedule that produced them ─────────
//
// Soft FK from `agentic_runs.schedule_id` → `agentic_schedules.id`. Both
// tables live in this runtime crate (not a domain), so this link is
// structural — the same flavor as `parent_run_id` — rather than a
// domain-specific column leaking onto the generic run row.
//
// Set by the scheduler at fire time (and by `run_now`) so:
//   * per-job run history queries can do `WHERE schedule_id = $1` with an
//     index instead of scanning all of `agentic_runs`,
//   * the dashboard timeline can match actual runs back to the schedule
//     that fired them and surface missed slots.
//
// Soft FK (no constraint) — consistent with `parent_run_id` / `workspace_id`.
// Index covers the dominant access pattern: newest runs for a given schedule.

struct AddRunScheduleId;

impl MigrationName for AddRunScheduleId {
    fn name(&self) -> &str {
        "m20260524_000001_add_run_schedule_id"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddRunScheduleId {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_runs", "schedule_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .add_column(ColumnDef::new(Alias::new("schedule_id")).string().null())
                        .to_owned(),
                )
                .await?;
        }
        manager
            .create_index(
                Index::create()
                    .name("idx_agentic_runs_schedule_id_created_at")
                    .table(AgenticRun::Table)
                    .col(Alias::new("schedule_id"))
                    .col((Alias::new("created_at"), IndexOrder::Desc))
                    .if_not_exists()
                    .to_owned(),
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_agentic_runs_schedule_id_created_at")
                    .table(AgenticRun::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await?;
        if column_exists(manager, "agentic_runs", "schedule_id").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticRun::Table)
                        .drop_column(Alias::new("schedule_id"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

// ── Migration 20: agentic_schedules.question (agent target kind) ──────────────
//
// Free-text question stored on the schedule row. Required when
// `target_kind = 'agent'` — each fire seeds an analytics run with this
// question; ignored for workflow / airway schedules. NULL allowed so
// existing rows back-fill cleanly.

struct AddScheduleQuestion;

impl MigrationName for AddScheduleQuestion {
    fn name(&self) -> &str {
        "m20260526_000001_add_schedule_question"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddScheduleQuestion {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_schedules", "question").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticSchedule::Table)
                        .add_column(ColumnDef::new(Alias::new("question")).text().null())
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_schedules", "question").await? {
            manager
                .alter_table(
                    Table::alter()
                        .table(AgenticSchedule::Table)
                        .drop_column(Alias::new("question"))
                        .to_owned(),
                )
                .await?;
        }
        Ok(())
    }
}

/// **Delayed visibility for the task queue** — the column that makes queueing
/// expressible.
///
/// `agentic_task_queue` could say "claimable now" (`queued`) or "someone has
/// it" (`claimed`), and nothing else. There was no way to say *"make this
/// claimable again in 30 seconds"*, so every deferral had to be spelled as a
/// claim that is then allowed to time out — which burns `claim_count` toward
/// `max_claims` and is indistinguishable from a worker that crashed. That is
/// why backoff, contention retry, and "wait your turn" were all hard to build
/// on this queue: the vocabulary was missing, not the machinery.
///
/// `available_at` defaults to `now()`, so every existing row and every INSERT
/// that does not mention it behaves exactly as before — this migration is
/// behaviour-neutral until a caller sets a future value.
///
/// The existing poll index (`(created_at) WHERE queue_status = 'queued'`) is
/// left as-is: it already returns rows in the order the claim needs, and the
/// added `available_at <= now()` test is a cheap filter applied as rows are
/// walked.
struct AddTaskQueueAvailableAt;

impl MigrationName for AddTaskQueueAvailableAt {
    fn name(&self) -> &str {
        "m20260810_000001_add_task_queue_available_at"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddTaskQueueAvailableAt {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_task_queue", "available_at").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    "ALTER TABLE agentic_task_queue \
                     ADD COLUMN available_at TIMESTAMPTZ NOT NULL DEFAULT now()",
                )
                .await?;
        }
        // The poll index is LEFT ALONE, deliberately.
        //
        // An earlier revision rebuilt it as `(available_at, created_at)` on the
        // theory that the filter column must lead. That is backwards here:
        // `available_at <= now()` is a RANGE, so a leading `available_at`
        // returns rows in `available_at` order and `ORDER BY created_at LIMIT 1`
        // then has to sort every available row. The existing `(created_at)`
        // index already returns rows in the required order and stops at the
        // first one passing the (cheap, usually-true) `available_at` test.
        //
        // It would only be worth revisiting if most queued rows were deferred,
        // which is not the shape here — deferral is the contended exception.
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_task_queue", "available_at").await? {
            manager
                .get_connection()
                .execute_unprepared("ALTER TABLE agentic_task_queue DROP COLUMN available_at")
                .await?;
        }
        Ok(())
    }
}

/// **Starvation detection for deferred tasks.**
///
/// `available_at` says when a task may next be claimed; it cannot say how long
/// it has been waiting, because each defer overwrites it. Without that, an
/// indefinitely-contended task waits forever in silence.
///
/// Wall clock, not a counter: the retry interval is chosen per domain and can
/// change, so N defers is not a bounded amount of time. `first_deferred_at`
/// records when the CURRENT waiting streak began — set on the first defer,
/// left alone by later ones, cleared when the task is (re-)enqueued as fresh
/// work.
struct AddTaskQueueFirstDeferredAt;

impl MigrationName for AddTaskQueueFirstDeferredAt {
    fn name(&self) -> &str {
        "m20260810_000002_add_task_queue_first_deferred_at"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddTaskQueueFirstDeferredAt {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if !column_exists(manager, "agentic_task_queue", "first_deferred_at").await? {
            manager
                .get_connection()
                .execute_unprepared(
                    "ALTER TABLE agentic_task_queue ADD COLUMN first_deferred_at TIMESTAMPTZ NULL",
                )
                .await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        if column_exists(manager, "agentic_task_queue", "first_deferred_at").await? {
            manager
                .get_connection()
                .execute_unprepared("ALTER TABLE agentic_task_queue DROP COLUMN first_deferred_at")
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm_migration::sea_orm::sea_query::PostgresQueryBuilder;

    /// `agentic_run_events.id` must render `bigserial`, not an identity column.
    ///
    /// This migrator is a separate crate from `crates/migration` with its own
    /// tracking table, so the twin assertion over there does not cover it. The
    /// protection used to be the workspace-wide `postgres-use-serial-pk`
    /// feature, which covered every call site at once; now that the type is
    /// spelled per site, each site needs its own assertion.
    #[test]
    fn agentic_run_events_id_renders_bigserial() {
        let ddl = agentic_run_events_table().to_string(PostgresQueryBuilder);

        // Full-statement equality, not `contains`. This statement and the one in
        // `crates/migration` are near-copies, and both are `IF NOT EXISTS`: add a
        // column to one and not the other and whichever migrator runs first wins
        // while the second silently no-ops, so the divergence surfaces as a
        // missing column at runtime rather than as a failed migration. Pinning
        // both renderings turns a one-sided edit into a failing test.
        //
        // The two are NOT identical and must not be asserted equal — this
        // migrator pins the plural `agentic_run_events` / `agentic_runs`, while
        // `crates/migration` derives the singular forms from its Iden enum
        // names. That difference is transient, not a second set of tables:
        // central's `m20260317_000002` renames the singular trio to plural right
        // after creating it, so by the time this migrator runs the plural tables
        // already exist and its `if_not_exists` create no-ops. A fresh database
        // ends up with three plural tables — the ones the entity layer reads.
        assert_eq!(
            ddl,
            r#"CREATE TABLE IF NOT EXISTS "agentic_run_events" ( "id" bigserial NOT NULL PRIMARY KEY, "run_id" varchar NOT NULL, "seq" bigint NOT NULL, "event_type" varchar NOT NULL, "payload" jsonb NOT NULL, "created_at" timestamp with time zone NOT NULL, FOREIGN KEY ("run_id") REFERENCES "agentic_runs" ("id") ON DELETE CASCADE )"#,
            "agentic_run_events DDL changed. Causes, in rough order of likelihood: \
             a column edit here (update this literal AND check whether \
             crates/migration's twin needs the same edit); or a sea-query \
             rendering change upstream (whitespace/quoting), in which case only \
             this literal needs refreshing."
        );
        assert!(
            !ddl.to_uppercase().contains("IDENTITY"),
            "identity columns diverge from every deployed database. Rendered: {ddl}"
        );
    }

    /// Negative control: `.auto_increment()` — the spelling the explicit
    /// `bigserial` replaced — must still render an identity column here.
    ///
    /// This crate needs its own copy rather than relying on the twin in
    /// `crates/migration`: cargo feature unification is per dependency graph, so
    /// `postgres-use-serial-pk` could be re-enabled for *this* crate (by a
    /// transitive dependency) while the other crate's control still passes. In
    /// that world the assertion above keeps passing on `bigserial` while proving
    /// nothing about the spelling it replaced.
    #[test]
    fn auto_increment_still_renders_identity_here() {
        let legacy = Table::create()
            .table(AgenticRunEvent::Table)
            .col(
                ColumnDef::new(AgenticRunEvent::Id)
                    .big_integer()
                    .not_null()
                    .auto_increment()
                    .primary_key(),
            )
            .to_string(PostgresQueryBuilder);

        assert!(
            legacy.to_uppercase().contains("IDENTITY"),
            "`.auto_increment()` no longer renders IDENTITY in this crate — either \
             `postgres-use-serial-pk` reached it (directly or transitively), or an \
             upstream sea-query default changed. Either way the explicit \
             `bigserial` above has stopped being load-bearing. Rendered: {legacy}"
        );
    }
    /// The other two tables this migrator shares with `crates/migration`.
    ///
    /// Same hazard as `agentic_run_events`: both crates create them with
    /// `IF NOT EXISTS`, so a column added on one side only does not fail — the
    /// second create no-ops and the column is simply missing at runtime.
    ///
    /// These pin the **create** shape only. `parent_run_id` / `task_status` /
    /// `task_metadata` are added to `agentic_runs` by later migrations in this
    /// file under `column_exists` guards; that is deliberate, not divergence.
    #[test]
    fn sibling_twin_tables_render_their_pinned_ddl() {
        assert_eq!(
            agentic_runs_table().to_string(PostgresQueryBuilder),
            r#"CREATE TABLE IF NOT EXISTS "agentic_runs" ( "id" varchar NOT NULL PRIMARY KEY, "agent_id" varchar NOT NULL, "question" text NOT NULL, "status" varchar NOT NULL DEFAULT 'running', "answer" text NULL, "error_message" text NULL, "created_at" timestamp with time zone NOT NULL, "updated_at" timestamp with time zone NOT NULL )"#,
            "agentic_runs DDL changed. Causes: a column edit here (update this \
             literal AND check crates/migration's twin); or an upstream \
             sea-query rendering change, in which case only this literal needs \
             refreshing."
        );

        assert_eq!(
            agentic_run_suspensions_table().to_string(PostgresQueryBuilder),
            r#"CREATE TABLE IF NOT EXISTS "agentic_run_suspensions" ( "run_id" varchar NOT NULL PRIMARY KEY, "prompt" text NOT NULL, "suggestions" jsonb NOT NULL, "resume_data" jsonb NOT NULL, "created_at" timestamp with time zone NOT NULL, FOREIGN KEY ("run_id") REFERENCES "agentic_runs" ("id") ON DELETE CASCADE )"#,
            "agentic_run_suspensions DDL changed. Causes: a column edit here \
             (update this literal AND check crates/migration's twin); or an \
             upstream sea-query rendering change."
        );
    }
    /// The partial index and the poll it exists for must agree on the statuses.
    ///
    /// Postgres uses a partial index only when it can prove the query implies
    /// the index predicate. Add a status to the poll without adding a NEW
    /// migration and that proof fails: the index is silently ignored and the
    /// poll goes back to a full index scan — 17ms per execution at ~13/second,
    /// which is exactly the load profile of the 2026-09-01 outage. Nothing
    /// errors; it just gets slow again.
    ///
    /// So the coupling is checked here rather than commented. This asserts the
    /// FROZEN migration literal against the LIVE constant: if you change the
    /// poll, this goes red and the fix is a new migration, never an edit to the
    /// shipped one.
    #[test]
    fn pending_global_index_covers_every_polled_status() {
        use crate::orchestrator::crud::recovery::PENDING_GLOBAL_STATUSES;

        for status in PENDING_GLOBAL_STATUSES {
            assert!(
                PENDING_GLOBAL_INDEX_SQL.contains(&format!("'{status}'")),
                "status `{status}` is polled by find_pending_global_runs but is \
                 NOT in idx_agentic_runs_pending_global's predicate. The index \
                 will be silently ignored. Add a NEW migration creating an index \
                 that covers it — do not edit the shipped one."
            );
        }
        // …and nothing extra: an index predicate wider than the query is not a
        // correctness problem, but it means this literal and the constant have
        // drifted, which is the thing this test exists to notice.
        let quoted = PENDING_GLOBAL_INDEX_SQL.matches('\'').count() / 2;
        assert_eq!(
            quoted,
            PENDING_GLOBAL_STATUSES.len(),
            "the index predicate lists {quoted} statuses but the poll uses {}",
            PENDING_GLOBAL_STATUSES.len()
        );
    }

    /// The shared unique index, pinned like the tables — and it is the sharper
    /// case: both migrators create `idx_agentic_run_events_run_id_seq` under
    /// the *same name*, so `IF NOT EXISTS` no-ops by name. Change the columns
    /// or drop `.unique()` on one side and the other silently does nothing.
    #[test]
    fn the_shared_index_renders_its_pinned_ddl() {
        assert_eq!(
            agentic_run_events_index().to_string(PostgresQueryBuilder),
            r#"CREATE UNIQUE INDEX IF NOT EXISTS "idx_agentic_run_events_run_id_seq" ON "agentic_run_events" ("run_id", "seq")"#,
            "the shared index changed. Causes: an edit here (update this literal \
             AND check crates/migration's twin, which creates the same index name); or an \
             upstream sea-query rendering change."
        );
    }
}

/// Partial index for `find_pending_global_runs` — the driver loops' poll.
///
/// That query has no `workspace_id` predicate (the loops pass `None`), so the
/// only usable index was `idx_agentic_runs_workspace_status`, whose leading
/// column it cannot constrain. Postgres therefore scanned the whole index:
/// measured in production at **16.95 ms to return 0 rows**, over 634,955 live
/// rows, executed ~13×/second across the fleet. That one query was 95% of all
/// database load — while finding nothing.
///
/// This index stores only the rows the poll can ever match — 15 of 634,955 at
/// the time of writing — so the same poll becomes a few microseconds and stays
/// flat as `agentic_runs` grows. The predicate is copied from the query
/// verbatim; if that `IN` list changes, this must change with it or the index
/// silently stops being used (a planner regression, not an error).
/// The **frozen** DDL for this migration.
///
/// A shipped migration cannot be edited, so this literal is deliberately a
/// duplicate of `crud::recovery::PENDING_GLOBAL_STATUSES` rather than derived
/// from it: deriving would let the index silently change shape for databases
/// that already ran it. The duplication is held honest by
/// `pending_global_index_covers_every_polled_status` — change the statuses the
/// poll uses and that test goes red, telling you to add a NEW migration.
const PENDING_GLOBAL_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_agentic_runs_pending_global \
     ON agentic_runs (task_status) \
     WHERE parent_run_id IS NULL \
       AND task_status IN ('running','delegating','waiting_on_child',\
           'waiting_on_children','needs_resume','shutdown')";

#[derive(DeriveMigrationName)]
pub struct AddPendingGlobalRunsIndex;

#[async_trait::async_trait]
impl MigrationTrait for AddPendingGlobalRunsIndex {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(PENDING_GLOBAL_INDEX_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_agentic_runs_pending_global")
            .await?;
        Ok(())
    }
}

/// Partial index for the lazy-compile backoff window.
///
/// `enqueue_compile_deduped` (oxy-app's workspace middleware) asks, on the
/// request hot path and while holding `pg_try_advisory_xact_lock`, for the
/// last few TERMINAL compile tasks of one workspace, newest first. Nothing
/// served that: `agentic_task_queue`'s only two indexes are partial on
/// `queued` and `claimed`, i.e. the non-terminal rows, so the planner fell
/// back to a sequential scan over every retained terminal row — 7 days of
/// `completed`/`cancelled` and 30 days of `failed`/`dead`, across every
/// workspace and every `TaskSpec` variant — followed by a sort.
///
/// `worker_metrics.rs` already carried the warning that this table "wants an
/// index covering the three statuses before it wants any other optimisation";
/// this is that index, shaped for the query that made it urgent.
///
/// The predicate stays on `spec->>'type'` alone rather than also pinning
/// `queue_status`, so the same index serves the window whatever the terminal
/// status list becomes, and stays useful if the sibling in-flight check is
/// ever widened. Both extractions are on a `jsonb` column, so they are
/// IMMUTABLE and legal in an index predicate and key.
const COMPILE_TASK_BACKOFF_INDEX_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_task_queue_compile_terminal \
     ON agentic_task_queue ((spec->>'workspace_id'), updated_at DESC) \
     WHERE spec->>'type' = 'compile'";

pub struct AddCompileTaskBackoffIndex;

/// Named by hand, NOT by `DeriveMigrationName`.
///
/// That derive takes the **module** path, not the struct name, so every
/// derived migration in this file would register as `"migration"` — the
/// second one to be added collides on `seaql_migrations_orchestrator_pkey`
/// and every migration run fails with a duplicate key. `AddPendingGlobalRunsIndex`
/// above is the one that got away with it by being the only derive here; leave
/// it as it is, since renaming a shipped migration re-runs it.
impl MigrationName for AddCompileTaskBackoffIndex {
    fn name(&self) -> &str {
        "m20260924_000001_add_compile_task_backoff_index"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddCompileTaskBackoffIndex {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(COMPILE_TASK_BACKOFF_INDEX_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_task_queue_compile_terminal")
            .await?;
        Ok(())
    }
}
