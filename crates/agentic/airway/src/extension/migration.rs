//! Airway extension migrations.
//!
//! Uses a separate tracking table (`seaql_migrations_airway`) so this
//! migrator evolves independently of the runtime, automation, and
//! analytics migrators. Run in the standard startup order after the
//! runtime migrator (which owns `agentic_runs`, the target of
//! `airway_run_extensions.run_id`'s FK).

use sea_orm_migration::prelude::*;

pub struct AirwayMigrator;

#[async_trait::async_trait]
impl MigratorTrait for AirwayMigrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![
            Box::new(CreatePipelineState),
            Box::new(CreateLoadAudit),
            Box::new(CreateRunExtensions),
            Box::new(AddPartialToLoadAudit),
            Box::new(AddRetryStateToRunExtensions),
            Box::new(CreatePipelineLeases),
            Box::new(AddAdmissionToRunExtensions),
            Box::new(CreateWorkspacePipelineState),
            Box::new(AddWorkspaceToLoadAudit),
        ]
    }

    fn migration_table_name() -> sea_orm::DynIden {
        Alias::new("seaql_migrations_airway").into_iden()
    }

    oxy_migration_tolerance::tolerate_schema_ahead!();
}

// ── Iden enums ───────────────────────────────────────────────────────────────

#[derive(Iden)]
enum AirwayPipelineState {
    #[iden = "airway_pipeline_state"]
    Table,
    PipelineName,
    State,
    SchemaJson,
    Version,
    UpdatedAt,
}

#[derive(Iden)]
enum AirwayLoadAudit {
    #[iden = "airway_load_audit"]
    Table,
    LoadId,
    WorkspaceId,
    PipelineName,
    SchemaHash,
    Status,
    ErrorMessage,
    Partial,
    StartedAt,
    FinishedAt,
}

#[derive(Iden)]
enum AirwayRunExtensions {
    #[iden = "airway_run_extensions"]
    Table,
    RunId,
    PipelineName,
    PipelineRef,
    LoadId,
    Concurrency,
    Resources,
    RetryCount,
    ResumeState,
    ContractPolicy,
    Environment,
}

#[derive(Iden)]
enum AirwayWorkspacePipelineState {
    #[iden = "airway_workspace_pipeline_state"]
    Table,
    WorkspaceId,
    PipelineName,
    State,
    SchemaJson,
    Version,
    UpdatedAt,
}

#[derive(Iden)]
enum AirwayPipelineLeases {
    #[iden = "airway_pipeline_leases"]
    Table,
    WorkspaceId,
    PipelineName,
    RunId,
    AcquiredAt,
    ExpiresAt,
}

/// Mirror of the runtime's `agentic_runs` table — only the `Id`
/// column is referenced for the FK target on `airway_run_extensions`.
#[derive(Iden)]
enum AgenticRun {
    #[iden = "agentic_runs"]
    Table,
    Id,
}

// ── Migration 1: airway_pipeline_state ──────────────────────────────────────

struct CreatePipelineState;

impl MigrationName for CreatePipelineState {
    fn name(&self) -> &str {
        "m20260520_000001_create_airway_pipeline_state"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreatePipelineState {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AirwayPipelineState::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AirwayPipelineState::PipelineName)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AirwayPipelineState::State)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AirwayPipelineState::SchemaJson)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AirwayPipelineState::Version)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(AirwayPipelineState::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(AirwayPipelineState::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 2: airway_load_audit ──────────────────────────────────────────

struct CreateLoadAudit;

impl MigrationName for CreateLoadAudit {
    fn name(&self) -> &str {
        "m20260520_000002_create_airway_load_audit"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateLoadAudit {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AirwayLoadAudit::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AirwayLoadAudit::LoadId)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AirwayLoadAudit::PipelineName)
                            .text()
                            .not_null(),
                    )
                    // Nullable: airway's `Schema.version_hash` is `Option<String>`.
                    // On the first-ever load for a pipeline the state store
                    // hasn't seen a schema yet, so there's no hash to record.
                    // NULL distinguishes "no prior schema" from "ran with an
                    // empty schema."
                    .col(ColumnDef::new(AirwayLoadAudit::SchemaHash).text().null())
                    .col(
                        ColumnDef::new(AirwayLoadAudit::Status)
                            .text()
                            .not_null()
                            .default("in_progress"),
                    )
                    .col(ColumnDef::new(AirwayLoadAudit::ErrorMessage).text().null())
                    .col(
                        ColumnDef::new(AirwayLoadAudit::StartedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(AirwayLoadAudit::FinishedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Common query: "audit history for this pipeline, newest first".
        manager
            .create_index(
                Index::create()
                    .name("idx_airway_load_audit_pipeline_name")
                    .table(AirwayLoadAudit::Table)
                    .col(AirwayLoadAudit::PipelineName)
                    .col(AirwayLoadAudit::StartedAt)
                    .if_not_exists()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(AirwayLoadAudit::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 3: airway_run_extensions ──────────────────────────────────────

struct CreateRunExtensions;

impl MigrationName for CreateRunExtensions {
    fn name(&self) -> &str {
        "m20260520_000003_create_airway_run_extensions"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateRunExtensions {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AirwayRunExtensions::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AirwayRunExtensions::RunId)
                            .text()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(AirwayRunExtensions::PipelineName)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AirwayRunExtensions::PipelineRef)
                            .text()
                            .null(),
                    )
                    .col(ColumnDef::new(AirwayRunExtensions::LoadId).text().null())
                    .col(
                        ColumnDef::new(AirwayRunExtensions::Concurrency)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(AirwayRunExtensions::Resources)
                            .json_binary()
                            .not_null()
                            .default("[]"),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .from(AirwayRunExtensions::Table, AirwayRunExtensions::RunId)
                            .to(AgenticRun::Table, AgenticRun::Id)
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
                    .table(AirwayRunExtensions::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 4: airway_load_audit.partial ──────────────────────────────────

/// Adds the `partial` flag distinguishing a clean load from a
/// completed-with-errors one (streaming per-batch commit). Separate
/// migration — never edit a shipped one — so both fresh and existing
/// databases converge.
struct AddPartialToLoadAudit;

impl MigrationName for AddPartialToLoadAudit {
    fn name(&self) -> &str {
        "m20260520_000004_add_partial_to_airway_load_audit"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddPartialToLoadAudit {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayLoadAudit::Table)
                    .add_column(
                        ColumnDef::new(AirwayLoadAudit::Partial)
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
                    .table(AirwayLoadAudit::Table)
                    .drop_column(AirwayLoadAudit::Partial)
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 5: airway_run_extensions.retry_count + resume_state ────────────

/// Adds per-run `retry_count` (the reset-in-place retry counter, surfaced in the
/// run UI) and `resume_state` (the run's persisted cursor, so a reset-in-place
/// retry resumes where it left off instead of re-extracting the window).
/// Separate migration — never edit a shipped one — so fresh and existing DBs
/// converge.
struct AddRetryStateToRunExtensions;

impl MigrationName for AddRetryStateToRunExtensions {
    fn name(&self) -> &str {
        "m20260520_000005_add_retry_state_to_airway_run_extensions"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddRetryStateToRunExtensions {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayRunExtensions::Table)
                    .add_column(
                        ColumnDef::new(AirwayRunExtensions::RetryCount)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayRunExtensions::Table)
                    .add_column(
                        ColumnDef::new(AirwayRunExtensions::ResumeState)
                            .json_binary()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayRunExtensions::Table)
                    .drop_column(AirwayRunExtensions::RetryCount)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayRunExtensions::Table)
                    .drop_column(AirwayRunExtensions::ResumeState)
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 6: airway_pipeline_leases ─────────────────────────────────────

/// Single-flight lease: at most one *active* airway run per
/// `(workspace_id, pipeline_name)`.
///
/// Two overlapping runs of one pipeline are not merely wasteful, they are
/// incorrect on two independent axes:
///
///  1. **Cursor corruption.** `airway_pipeline_state` is keyed by
///     `pipeline_name`, so concurrent runs read-modify-write a single cursor
///     row. The optimistic `version` check makes the loser *fail its save*
///     rather than merge — leaving a window silently skipped or re-pulled.
///  2. **Duplicate rows downstream.** Each run ends with a merge-on-read fold
///     of `<schema>_raw.<table>` into the served table. Two folds whose snapshots
///     overlap each purge against a base the other has not committed yet, so
///     both versions of a changed row survive. Measured on pokehouse
///     (2026-08-05): 34 excess rows in `toast_pos.orders`, 104 in
///     `order_selections` — every duplicate pair spanning two `_aw_load_id`s.
///
/// Keyed by workspace **and** name because `pipeline_name` comes from the
/// YAML and is not globally unique — two tenants may both ship a pipeline
/// called `restaurant_analytics`, and one must never gate the other.
///
/// `expires_at` (not a liveness heartbeat) is the crash backstop: a worker
/// that OOMs mid-run cannot release, so the lease self-heals at expiry. That
/// mirrors the task queue's own reaper, which reclaims stale claims on the
/// same principle rather than tracking worker liveness.
struct CreatePipelineLeases;

impl MigrationName for CreatePipelineLeases {
    fn name(&self) -> &str {
        "m20260805_000006_create_airway_pipeline_leases"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreatePipelineLeases {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AirwayPipelineLeases::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AirwayPipelineLeases::WorkspaceId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AirwayPipelineLeases::PipelineName)
                            .text()
                            .not_null(),
                    )
                    // The run currently holding the lease. Returned to the
                    // caller on a conflict so the UI can link to the run that
                    // is already in flight instead of just saying "busy".
                    .col(
                        ColumnDef::new(AirwayPipelineLeases::RunId)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AirwayPipelineLeases::AcquiredAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(AirwayPipelineLeases::ExpiresAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    // Composite PK is what makes acquisition atomic: the
                    // `ON CONFLICT` target below is this constraint, so two
                    // replicas racing to start the same pipeline resolve in
                    // the database rather than in a check-then-act window.
                    .primary_key(
                        Index::create()
                            .col(AirwayPipelineLeases::WorkspaceId)
                            .col(AirwayPipelineLeases::PipelineName),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(AirwayPipelineLeases::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 7: admission policy on airway_run_extensions ──────────────────

/// Adds `contract_policy` and `environment` — the two admission strings this
/// run was resolved with at enqueue (see `agentic_pipeline::airway_config::
/// resolve_admission`). Stage 2 moved airway's admission policy from git into
/// Postgres, which means a config row can change after the fact; these
/// columns are what lets a past run still say what admitted it. `NULL` on
/// both means "airway's own default" (`permissive` / `production`), same as
/// an unconfigured source today. Separate migration — never edit a shipped
/// one — so fresh and existing DBs converge.
struct AddAdmissionToRunExtensions;

impl MigrationName for AddAdmissionToRunExtensions {
    fn name(&self) -> &str {
        "m20260805_000007_add_admission_to_airway_run_extensions"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddAdmissionToRunExtensions {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayRunExtensions::Table)
                    .add_column(
                        ColumnDef::new(AirwayRunExtensions::ContractPolicy)
                            .string()
                            .null(),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayRunExtensions::Table)
                    .add_column(
                        ColumnDef::new(AirwayRunExtensions::Environment)
                            .string()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayRunExtensions::Table)
                    .drop_column(AirwayRunExtensions::ContractPolicy)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayRunExtensions::Table)
                    .drop_column(AirwayRunExtensions::Environment)
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 8: airway_workspace_pipeline_state ────────────────────────────
//
// `airway_pipeline_state` is keyed by `pipeline_name` alone, so two workspaces
// running a pipeline of the same name share one cursor and one stored schema:
// the second resumes from the first's position and skips rows, and a reset in
// either wipes both. A NEW table rather than a re-key of the old one, because
// an old pod's `ON CONFLICT (pipeline_name)` needs the name to stay unique
// while a rolling deploy is in flight. The store adopts the legacy row on
// first read, so no cursor is lost; the legacy table is retired once no
// adoption has been logged (target `airway_state`) for a full scheduling cycle
// across the fleet.
// Design: internal-docs/airway-single-flight.md.

struct CreateWorkspacePipelineState;

impl MigrationName for CreateWorkspacePipelineState {
    fn name(&self) -> &str {
        "m20260916_000001_create_airway_workspace_pipeline_state"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for CreateWorkspacePipelineState {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(AirwayWorkspacePipelineState::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(AirwayWorkspacePipelineState::WorkspaceId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AirwayWorkspacePipelineState::PipelineName)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(AirwayWorkspacePipelineState::State)
                            .json_binary()
                            .not_null(),
                    )
                    // Nullable, unlike the legacy table: a reset writes a
                    // tombstone row (default state, no schema) instead of
                    // deleting, so the next load does not adopt the legacy
                    // cursor back. NULL here means "no schema provisioned",
                    // which is exactly what an absent row used to mean.
                    .col(
                        ColumnDef::new(AirwayWorkspacePipelineState::SchemaJson)
                            .json_binary()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(AirwayWorkspacePipelineState::Version)
                            .big_integer()
                            .not_null()
                            .default(0),
                    )
                    .col(
                        ColumnDef::new(AirwayWorkspacePipelineState::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .col(AirwayWorkspacePipelineState::WorkspaceId)
                            .col(AirwayWorkspacePipelineState::PipelineName),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(AirwayWorkspacePipelineState::Table)
                    .if_exists()
                    .to_owned(),
            )
            .await
    }
}

// ── Migration 9: airway_load_audit.workspace_id ─────────────────────────────
//
// Append-only and keyed by `load_id`, so the audit needs a column rather than
// a new table. New rows always set it.
//
// Old rows are backfilled best-effort through `airway_run_extensions.load_id`,
// which is indexed on both sides and covers successful loads since 2026-07-22.
// Everything else stays NULL, and that is deliberate: the other route to a
// workspace is the run's `load_started` event, and matching it needs
// `event_type = 'load_started' AND payload->>'load_id' = …` against
// `agentic_run_events`, whose only unique index is `(run_id, seq)`. That is a
// full scan of every event of every agentic run in the deployment, run by
// `AirwayMigrator::up` *before* serve binds its listener — a boot stall
// proportional to the whole product's event history, to populate a nullable
// column nothing reads yet. If those rows are ever wanted, backfill them from
// an ops command against a live database, not from the boot path.

struct AddWorkspaceToLoadAudit;

impl MigrationName for AddWorkspaceToLoadAudit {
    fn name(&self) -> &str {
        "m20260916_000002_add_workspace_to_airway_load_audit"
    }
}

#[async_trait::async_trait]
impl MigrationTrait for AddWorkspaceToLoadAudit {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayLoadAudit::Table)
                    .add_column(ColumnDef::new(AirwayLoadAudit::WorkspaceId).uuid().null())
                    .to_owned(),
            )
            .await?;
        manager
            .create_index(
                Index::create()
                    .if_not_exists()
                    .name("idx_airway_load_audit_workspace_pipeline")
                    .table(AirwayLoadAudit::Table)
                    .col(AirwayLoadAudit::WorkspaceId)
                    .col(AirwayLoadAudit::PipelineName)
                    .col(AirwayLoadAudit::StartedAt)
                    .to_owned(),
            )
            .await?;

        // Best-effort backfill through the run that owns each load. The runtime
        // migrator owns `agentic_runs` and runs before this one, so the join is
        // safe. Bounded by the SIZE of what it touches, not by indexes: this
        // scans `airway_run_extensions` (one row per airway run) and probes two
        // primary keys, `airway_load_audit.load_id` and `agentic_runs.id`.
        // `airway_run_extensions.load_id` carries no index — that is fine at one
        // row per airway run, and is the whole difference from the event-log
        // route declined above, which would scan every event of every agentic
        // run in the deployment. Rows with no extension row keep a NULL
        // workspace — see the note above.
        let db = manager.get_connection();
        db.execute_unprepared(
            "UPDATE airway_load_audit a
                SET workspace_id = r.workspace_id
               FROM airway_run_extensions e
               JOIN agentic_runs r ON r.id = e.run_id
              WHERE e.load_id = a.load_id
                AND a.workspace_id IS NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_airway_load_audit_workspace_pipeline")
                    .table(AirwayLoadAudit::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(AirwayLoadAudit::Table)
                    .drop_column(AirwayLoadAudit::WorkspaceId)
                    .to_owned(),
            )
            .await
    }
}
