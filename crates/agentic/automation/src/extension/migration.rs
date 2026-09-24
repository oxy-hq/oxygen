//! Automation extension migrations.
//!
//! Uses a separate tracking table (`seaql_migrations_workflow`) so this
//! migrator is independent of the runtime and analytics migrators.

use sea_orm_migration::prelude::*;

pub struct AutomationMigrator;

#[async_trait::async_trait]
impl MigratorTrait for AutomationMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(CreateAutomationState),
            Box::new(AddStepHashes),
            Box::new(AddRetryLinkage),
            Box::new(AddPriorSnapshot),
            Box::new(AddInitialRenderContext),
            Box::new(AddInvalidateIterations),
        ]
    }

    fn migration_table_name() -> sea_orm::DynIden {
        Alias::new("seaql_migrations_workflow").into_iden()
    }

    oxy_migration_tolerance::tolerate_schema_ahead!();
}

// ── Iden ─────────────────────────────────────────────────────────────────────

#[derive(Iden)]
enum AutomationState {
    #[iden = "agentic_workflow_state"]
    Table,
    RunId,
    WorkflowYamlHash,
    WorkflowConfig,
    WorkflowContext,
    Variables,
    TraceId,
    CurrentStep,
    Results,
    RenderContext,
    PendingChildren,
    DecisionVersion,
    UpdatedAt,
    StepHashes,
    RetryFromRunId,
    CacheEnabled,
    PriorStepHashes,
    PriorResults,
    InitialRenderContext,
    InvalidateIterations,
}

#[derive(Iden)]
enum AgenticRun {
    #[iden = "agentic_runs"]
    Table,
    Id,
}

// ── Migration 1: Create agentic_workflow_state ────────────────────────────────

struct CreateAutomationState;

impl MigrationName for CreateAutomationState {
    fn name(&self) -> &str {
        "m20260416_000001_create_agentic_workflow_state"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateAutomationState {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AutomationState::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AutomationState::RunId)
                            .string()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AutomationState::WorkflowYamlHash)
                            .string()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AutomationState::WorkflowConfig)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AutomationState::WorkflowContext)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AutomationState::Variables)
                            .json_binary()
                            .null(),
                    )
                    .col(ColumnDef::new(AutomationState::TraceId).string().not_null())
                    .col(
                        ColumnDef::new(AutomationState::CurrentStep)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(AutomationState::Results)
                            .json_binary()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(AutomationState::RenderContext)
                            .json_binary()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(AutomationState::PendingChildren)
                            .json_binary()
                            .not_null()
                            .default("{}"),
                    )
                    .col(
                        ColumnDef::new(AutomationState::DecisionVersion)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(AutomationState::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AutomationState::Table, AutomationState::RunId)
                            .to(AgenticRun::Table, AgenticRun::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(AutomationState::Table).to_owned())
            .await
    }
}

// ── Migration 2: Add step_hashes column ──────────────────────────────────────

struct AddStepHashes;

impl MigrationName for AddStepHashes {
    fn name(&self) -> &str {
        "m20260507_000001_add_step_hashes"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddStepHashes {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .add_column(
                        ColumnDef::new(AutomationState::StepHashes)
                            .json_binary()
                            .not_null()
                            .default("{}"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .drop_column(AutomationState::StepHashes)
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 3: Add retry_from_run_id + cache_enabled columns ───────────────

/// Linkage for "resume only unchanged steps."
///
/// `retry_from_run_id` points at the prior run whose step results may be
/// reused — `NULL` means a fresh run with nothing to compare against.
/// `cache_enabled` is the explicit user opt-in: even with a prior-run pointer
/// set, the decider only consults the cache when this flag is true. Defaults
/// `FALSE` so existing rows and any caller that doesn't set it explicitly
/// gets the old fresh-execute behavior.
struct AddRetryLinkage;

impl MigrationName for AddRetryLinkage {
    fn name(&self) -> &str {
        "m20260507_000002_add_retry_linkage"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddRetryLinkage {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .add_column(
                        ColumnDef::new(AutomationState::RetryFromRunId)
                            .string()
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(AutomationState::CacheEnabled)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .drop_column(AutomationState::RetryFromRunId)
                    .drop_column(AutomationState::CacheEnabled)
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 4: Add prior-state snapshot columns ────────────────────────────

/// Pre-materialised cache snapshot.
///
/// At seed time the executor copies the prior run's `step_hashes` and `results`
/// into these columns, after stripping any entries named in `metadata.invalidate_steps`
/// on `agentic_runs`. The decider then consults this snapshot directly via the
/// in-memory `AutomationRunState` it already loaded for the current run — no
/// per-decision re-load of the prior row and no re-read of the invalidate list.
///
/// Both columns default to `'{}'` so existing rows that pre-date this
/// migration deserialize cleanly (they'll have no prior snapshot — equivalent
/// to the old behavior when `retry_from_run_id` is `NULL` / `cache_enabled = FALSE`).
struct AddPriorSnapshot;

impl MigrationName for AddPriorSnapshot {
    fn name(&self) -> &str {
        "m20260511_000001_add_prior_snapshot"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddPriorSnapshot {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .add_column(
                        ColumnDef::new(AutomationState::PriorStepHashes)
                            .json_binary()
                            .not_null()
                            .default("{}"),
                    )
                    .add_column(
                        ColumnDef::new(AutomationState::PriorResults)
                            .json_binary()
                            .not_null()
                            .default("{}"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .drop_column(AutomationState::PriorStepHashes)
                    .drop_column(AutomationState::PriorResults)
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 5: Add initial_render_context column ───────────────────────────

/// Seed-time render-context snapshot for synthetic sub-workflows.
///
/// When a `loop_sequential` step fans out, each iteration spawns a fresh
/// workflow run via `TaskSpec::Automation { body, initial_render_context }`.
/// The iteration variable (`{step_name}.value` / `.index`) plus the
/// parent run's accumulated render context land in
/// `initial_render_context` so inner template references like
/// `{{ analyze_each_store.value }}` resolve to the iteration's value.
///
/// Without persistence, the in-memory seed value was lost: the
/// vestigial `render_context` column is hardcoded to `'{}'` at insert
/// and rebuilt from `results` at load — neither path preserves the
/// iteration variable, so the inner SQL rendered with empty
/// substitutions ("`WHERE Store = `") and failed at parse time.
///
/// This column stores the seed value once and is read into every
/// `AutomationRunState::render_context` at load time, merged with the
/// rebuilt-from-results context. Defaults to `'{}'` so existing rows
/// (and non-loop runs) deserialize cleanly.
struct AddInitialRenderContext;

impl MigrationName for AddInitialRenderContext {
    fn name(&self) -> &str {
        "m20260512_000001_add_initial_render_context"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddInitialRenderContext {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .add_column(
                        ColumnDef::new(AutomationState::InitialRenderContext)
                            .json_binary()
                            .not_null()
                            .default("{}"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .drop_column(AutomationState::InitialRenderContext)
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 6: Add invalidate_iterations column ────────────────────────────

/// Per-step iteration indices the caller asked to force-replay on this
/// retry, ignoring whatever per-iteration cache entry the prior run
/// produced for that `(step, index)`. Shape: `{step_name: [indices]}`,
/// JSONB on the workflow state row.
///
/// Stamped onto the row once at seed time (read from
/// `agentic_runs.metadata.invalidate_iterations` by the executor) so
/// the decider's loop-step branch can apply it without re-reading
/// `agentic_runs` on every decision pass. Empty (`'{}'`) for runs
/// that don't pass the field.
struct AddInvalidateIterations;

impl MigrationName for AddInvalidateIterations {
    fn name(&self) -> &str {
        "m20260513_000001_add_invalidate_iterations"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddInvalidateIterations {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .add_column(
                        ColumnDef::new(AutomationState::InvalidateIterations)
                            .json_binary()
                            .not_null()
                            .default("{}"),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AutomationState::Table)
                    .drop_column(AutomationState::InvalidateIterations)
                    .to_owned(),
            )
            .await
    }
}
