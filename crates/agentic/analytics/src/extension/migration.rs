//! Analytics extension migrations.
//!
//! Uses a separate tracking table (`seaql_migrations_analytics`) so this
//! migrator is independent of the runtime and central migrators.

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};
use sea_orm_migration::prelude::*;

pub struct AnalyticsMigrator;

#[async_trait::async_trait]
impl MigratorTrait for AnalyticsMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(CreateAnalyticsRunExtensions),
            Box::new(BackfillFromLegacyColumns),
            Box::new(AddExecutionStamp),
        ]
    }

    fn migration_table_name() -> sea_orm::DynIden {
        Alias::new("seaql_migrations_analytics").into_iden()
    }

    oxy_migration_tolerance::tolerate_schema_ahead!();
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

// ── Iden ─────────────────────────────────────────────────────────────────────

#[derive(Iden)]
enum AnalyticsRunExtension {
    #[iden = "analytics_run_extensions"]
    Table,
    RunId,
    AgentId,
    SpecHint,
    ThinkingMode,
    ExecutionStartedAt,
    ExecutionHeartbeatAt,
}

#[derive(Iden)]
enum AgenticRun {
    #[iden = "agentic_runs"]
    Table,
    Id,
}

// ── Migration 1: Create table ───────────────────────────────────────────────

struct CreateAnalyticsRunExtensions;

impl MigrationName for CreateAnalyticsRunExtensions {
    fn name(&self) -> &str {
        "m20260407_000001_create_analytics_run_extensions"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateAnalyticsRunExtensions {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AnalyticsRunExtension::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AnalyticsRunExtension::RunId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AnalyticsRunExtension::AgentId)
                            .string()
                            .not_null()
                            .default(""),
                    )
                    .col(
                        ColumnDef::new(AnalyticsRunExtension::SpecHint)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AnalyticsRunExtension::ThinkingMode)
                            .string()
                            .null(),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AnalyticsRunExtension::Table, AnalyticsRunExtension::RunId)
                            .to(AgenticRun::Table, AgenticRun::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AnalyticsRunExtension::Table).to_owned())
            .await
    }
}

// ── Migration 2: Backfill from legacy columns ───────────────────────────────

struct BackfillFromLegacyColumns;

impl MigrationName for BackfillFromLegacyColumns {
    fn name(&self) -> &str {
        "m20260407_000002_backfill_analytics_extensions"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for BackfillFromLegacyColumns {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Only backfill if the source table exists and the legacy columns
        // haven't been dropped yet. On fresh installs or after the runtime
        // drop-columns migration, this is a no-op.
        if !table_exists(manager, "agentic_runs").await? {
            return Ok(());
        }
        if !column_exists(manager, "agentic_runs", "agent_id").await? {
            return Ok(());
        }

        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO analytics_run_extensions (run_id, agent_id, spec_hint, thinking_mode) \
                 SELECT id, agent_id, spec_hint, thinking_mode \
                 FROM agentic_runs \
                 WHERE agent_id != '__builder__' \
                 ON CONFLICT (run_id) DO NOTHING",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, _manager: &SchemaManager) -> Result<(), DbErr> {
        // Backfill is not reversible — the legacy columns still have the data.
        Ok(())
    }
}

// ── Migration 3: Execution stamp + heartbeat ────────────────────────────────

/// Adds `execution_started_at` — the moment an attempt began executing the
/// run — and `execution_heartbeat_at`, that attempt's proof it is still alive.
///
/// A run started from the task queue can be claimed more than once, and an
/// analytics run keeps no checkpoint until it suspends, so a second attempt
/// would start it again from the top. The stamp is what lets that attempt
/// refuse; the heartbeat is what tells it whether the first is dead or still
/// running. See `extension::execution` for the statements that write them.
///
/// Why columns here and not the queue's `claim_count` or the driver lease: a
/// graceful release *decrements* the count and it moves at claim time, before
/// anything ran; and a claim can be handed on under a driver whose lease never
/// lapsed. The same reasoning as `customer_app_automation_runs`, whose columns
/// these mirror.
///
/// Both nullable with no default, so a binary from before this migration —
/// which names its columns on insert — keeps inserting, and one rolled back
/// past it never reads them. No route serves either.
struct AddExecutionStamp;

impl MigrationName for AddExecutionStamp {
    fn name(&self) -> &str {
        "m20261006_000001_add_execution_stamp_to_analytics_run_extensions"
    }
}

const EXTENSIONS_TABLE: &str = "analytics_run_extensions";

/// Both columns, as `(name, iden)`: each is added and dropped the same way.
fn execution_columns() -> [(&'static str, AnalyticsRunExtension); 2] {
    [
        (
            "execution_started_at",
            AnalyticsRunExtension::ExecutionStartedAt,
        ),
        (
            "execution_heartbeat_at",
            AnalyticsRunExtension::ExecutionHeartbeatAt,
        ),
    ]
}

#[async_trait::async_trait]
impl MigrationTrait for AddExecutionStamp {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, column) in execution_columns() {
            if !column_exists(manager, EXTENSIONS_TABLE, name).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(AnalyticsRunExtension::Table)
                            .add_column(ColumnDef::new(column).timestamp_with_time_zone().null())
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        for (name, column) in execution_columns() {
            if column_exists(manager, EXTENSIONS_TABLE, name).await? {
                manager
                    .alter_table(
                        Table::alter()
                            .table(AnalyticsRunExtension::Table)
                            .drop_column(column)
                            .to_owned(),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
