//! The partial index behind the lazy-compile backoff window (moved out of
//! `migration.rs` unchanged).

use sea_orm_migration::prelude::*;

/// Partial index for the lazy-compile backoff window.
///
/// `enqueue_compile_deduped` (oxy-compile-runtime's `reconcile::enqueue`) asks, on the
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

/// Ships in the 0.5.154 release. It was held back from the 0.5.153 *release*
/// (tag `0.5.153`), the carrier for ledger tolerance: 0.5.152 cannot tolerate a
/// ledger row it does not know, so that release had to add none. `main` builds
/// after the cut report 0.5.153 and do register it. See
/// `internal-docs/revert-safe-migrations.md`.
pub struct AddCompileTaskBackoffIndex;

/// Named by hand, NOT by `DeriveMigrationName`.
///
/// That derive takes the **module** path, not the struct name, so every
/// derived migration in `migration.rs` would register as `"migration"` — the
/// second one to be added collides on `seaql_migrations_orchestrator_pkey`
/// and every migration run fails with a duplicate key. `AddPendingGlobalRunsIndex`
/// there is the one that got away with it by being the only derive; leave
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
