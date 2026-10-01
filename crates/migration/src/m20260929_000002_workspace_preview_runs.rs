//! `workspace_preview_runs` — the work Oxy staff start on a previewed branch
//! (`server::previews`): the Airway change check (`analyze`), and — later —
//! procedure dry runs, transform builds, Airway samples and compares.
//!
//! Control plane only: which revision, preview key and target a run reads, and
//! where it stands. A run's result rides its `agentic_runs` row (same id), and
//! holds ids, names and counts, never rows.
//!
//! The `kind` list is the whole phase 2 set on purpose, so the slices after the
//! change check reuse this table unchanged. Two partial unique indexes carry
//! the invariants the code leans on:
//!
//! - `one_analyze`: exactly one change check per revision, whoever asks first
//!   (the compile worker, or a create/refresh that reused a ready revision).
//! - `one_running`: at most one run that can write per workspace at a time.
//!
//! Purely additive: a new table nothing older reads or writes.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS workspace_preview_runs (
    run_id TEXT PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    branch TEXT NOT NULL,
    preview_key TEXT NOT NULL,
    revision_id UUID NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('analyze','procedure','transform_build','airway_sample','compare')),
    target_ref TEXT,
    parent_run_id TEXT,
    options JSONB NOT NULL DEFAULT '{}'::jsonb,
    state TEXT NOT NULL CHECK (state IN ('queued','running','finished')),
    requested_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_preview_runs_one_running ON workspace_preview_runs (workspace_id)
    WHERE state = 'running' AND kind IN ('procedure','transform_build','airway_sample');
CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_preview_runs_one_analyze ON workspace_preview_runs (workspace_id, revision_id)
    WHERE kind = 'analyze';
CREATE INDEX IF NOT EXISTS idx_workspace_preview_runs_queued ON workspace_preview_runs (workspace_id, created_at)
    WHERE state = 'queued';
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS workspace_preview_runs;")
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::UP_SQL;

    /// Later slices insert these kinds into the same table; a narrower CHECK
    /// here would make each of them a constraint change on a table something
    /// is already writing, which the rollback guard refuses.
    #[test]
    fn workspace_preview_runs_kind_check_names_every_phase_2_run_kind() {
        for kind in [
            "'analyze'",
            "'procedure'",
            "'transform_build'",
            "'airway_sample'",
            "'compare'",
        ] {
            assert!(UP_SQL.contains(kind), "kind CHECK is missing {kind}");
        }
    }

    /// At most one run that can write Airhouse (a procedure, a transform build
    /// or an Airway sample) runs per workspace at a time; the change check
    /// (`analyze`) and `compare` only read, so they are outside it.
    #[test]
    fn one_running_preview_run_per_workspace() {
        let flat = UP_SQL.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_preview_runs_one_running \
             ON workspace_preview_runs (workspace_id) \
             WHERE state = 'running' AND kind IN ('procedure','transform_build','airway_sample');"
        ));
    }

    /// `previews::analyze::ensure_enqueued` is idempotent only because this
    /// index exists: its `ON CONFLICT` infers it by columns and predicate.
    #[test]
    fn workspace_preview_runs_one_analyze_per_revision_is_a_partial_unique_index() {
        let flat = UP_SQL.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(flat.contains(
            "CREATE UNIQUE INDEX IF NOT EXISTS idx_workspace_preview_runs_one_analyze \
             ON workspace_preview_runs (workspace_id, revision_id) WHERE kind = 'analyze';"
        ));
    }
}
