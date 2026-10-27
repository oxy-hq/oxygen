//! Experiments become records: `experiments` (one per name per workspace) and
//! `experiment_designs` (insert-only versions of the whole `ExperimentConfig`).
//! A run pins one version (`design_id`), a proposal prices one, and the
//! `.experiment.yml` compile boundary (`experiment_definitions`) goes away. See
//! `internal-docs/experiments.md` "The artifact".
//!
//! Revert-safe in the sense `internal-docs/revert-safe-migrations.md` asks:
//! every statement is idempotent (`IF [NOT] EXISTS`, the backfill guarded on the
//! columns it reads), so a roll-forward re-run is a no-op. It contracts
//! `experiment_runs`, `experiment_proposals` and drops `experiment_definitions`,
//! which `migration_rollback_safety.rs` declares: every one of those was created
//! by an `m20261005_*` migration that has never reached a deploy, and ships to
//! main together with this one, so no previous deploy reads or writes them.
//!
//! Existing runs are kept: each becomes an experiment with its pinned design as
//! version 1. A backfilled row has no author, so `created_by` /
//! `registered_by` are the nil uuid, and its `content_hash` is over Postgres's
//! `jsonb` text rather than the canonical JSON the platform hashes — it never
//! equals a platform hash, which costs only a deduplication. A definition with
//! no run is not carried: it is a draft, and its file is still in git.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const CREATE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS experiments (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL,
    name TEXT NOT NULL,
    hypothesis TEXT NOT NULL,
    owner TEXT NOT NULL,
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT experiments_workspace_name_key UNIQUE (workspace_id, name)
);

CREATE TABLE IF NOT EXISTS experiment_designs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    -- No cascade: a design is evidence a run is pinned to.
    experiment_id UUID NOT NULL REFERENCES experiments(id),
    version INTEGER NOT NULL,
    parent_id UUID NULL REFERENCES experiment_designs(id),
    -- No FK, like every proposal reference: the log outlives what it priced.
    from_proposal_id UUID NULL,
    config JSONB NOT NULL,
    content_hash TEXT NOT NULL,
    created_by UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT experiment_designs_experiment_version_key UNIQUE (experiment_id, version)
);

-- Insert-only, enforced by the database: a version and a sealed result are
-- evidence, so an UPDATE is an error whoever issues it.
CREATE OR REPLACE FUNCTION experiment_refuse_update() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    RAISE EXCEPTION '% is insert-only: a row is never updated', TG_TABLE_NAME
        USING ERRCODE = 'restrict_violation';
END
$$;

DROP TRIGGER IF EXISTS experiment_designs_insert_only ON experiment_designs;
CREATE TRIGGER experiment_designs_insert_only BEFORE UPDATE ON experiment_designs
    FOR EACH ROW EXECUTE FUNCTION experiment_refuse_update();

DROP TRIGGER IF EXISTS experiment_results_insert_only ON experiment_results;
CREATE TRIGGER experiment_results_insert_only BEFORE UPDATE ON experiment_results
    FOR EACH ROW EXECUTE FUNCTION experiment_refuse_update();

ALTER TABLE experiment_proposals ADD COLUMN IF NOT EXISTS design_id UUID NULL;
ALTER TABLE experiment_proposals DROP COLUMN IF EXISTS revision_id;

ALTER TABLE experiment_runs ADD COLUMN IF NOT EXISTS experiment_id UUID NULL;
ALTER TABLE experiment_runs ADD COLUMN IF NOT EXISTS design_id UUID NULL;
ALTER TABLE experiment_runs ADD COLUMN IF NOT EXISTS registered_by UUID NULL;
"#;

/// Each existing run becomes an experiment with its pinned design as version 1.
/// Dynamic SQL inside the guard: once `config` is dropped the statement would not
/// even parse, and a re-run must be a no-op.
const BACKFILL_SQL: &str = r#"
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.columns
               WHERE table_schema = current_schema()
                 AND table_name = 'experiment_runs' AND column_name = 'config') THEN
        EXECUTE $q$
            INSERT INTO experiments (workspace_id, name, hypothesis, owner, created_by, created_at)
            SELECT r.workspace_id, r.name, COALESCE(r.config->>'hypothesis', ''),
                   COALESCE(r.config->>'owner', ''), '00000000-0000-0000-0000-000000000000',
                   r.created_at
            FROM experiment_runs r
            ON CONFLICT (workspace_id, name) DO NOTHING
        $q$;
        EXECUTE $q$
            INSERT INTO experiment_designs (experiment_id, version, config, content_hash,
                                            created_by, created_at)
            SELECT e.id, 1, r.config, encode(sha256(convert_to(r.config::text, 'UTF8')), 'hex'),
                   '00000000-0000-0000-0000-000000000000', r.registered_at
            FROM experiment_runs r
            JOIN experiments e ON e.workspace_id = r.workspace_id AND e.name = r.name
            WHERE r.design_id IS NULL
            ON CONFLICT (experiment_id, version) DO NOTHING
        $q$;
        EXECUTE $q$
            UPDATE experiment_runs r
            SET experiment_id = e.id, design_id = d.id,
                registered_by = '00000000-0000-0000-0000-000000000000'
            FROM experiments e
            JOIN experiment_designs d ON d.experiment_id = e.id AND d.version = 1
            WHERE e.workspace_id = r.workspace_id AND e.name = r.name AND r.design_id IS NULL
        $q$;
    END IF;
END
$$;
"#;

const CONTRACT_SQL: &str = r#"
ALTER TABLE experiment_runs ALTER COLUMN experiment_id SET NOT NULL;
ALTER TABLE experiment_runs ALTER COLUMN design_id SET NOT NULL;
ALTER TABLE experiment_runs ALTER COLUMN registered_by SET NOT NULL;
-- One run per experiment, ever: a registered design is immutable, and a new
-- design is a new version — not a second run.
CREATE UNIQUE INDEX IF NOT EXISTS uq_experiment_runs_experiment
    ON experiment_runs (experiment_id);
DROP INDEX IF EXISTS uq_experiment_runs_workspace_name;
ALTER TABLE experiment_runs DROP COLUMN IF EXISTS name;
ALTER TABLE experiment_runs DROP COLUMN IF EXISTS revision_id;
ALTER TABLE experiment_runs DROP COLUMN IF EXISTS config;

DROP TABLE IF EXISTS experiment_definitions;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(CREATE_SQL).await?;
        db.execute_unprepared(BACKFILL_SQL).await?;
        db.execute_unprepared(CONTRACT_SQL).await?;
        Ok(())
    }

    /// Schema only: the dropped columns come back empty and nullable, and
    /// `experiment_definitions` comes back empty — a contraction's data is gone.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
CREATE TABLE IF NOT EXISTS experiment_definitions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    workspace_id UUID NOT NULL,
    revision_id UUID NOT NULL REFERENCES revisions(revision_id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    name TEXT NOT NULL,
    config JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT experiment_definitions_revision_name_key UNIQUE (revision_id, name)
);
ALTER TABLE experiment_runs ADD COLUMN IF NOT EXISTS name TEXT NULL;
ALTER TABLE experiment_runs ADD COLUMN IF NOT EXISTS revision_id UUID NULL;
ALTER TABLE experiment_runs ADD COLUMN IF NOT EXISTS config JSONB NULL;
DROP INDEX IF EXISTS uq_experiment_runs_experiment;
ALTER TABLE experiment_runs DROP COLUMN IF EXISTS experiment_id;
ALTER TABLE experiment_runs DROP COLUMN IF EXISTS design_id;
ALTER TABLE experiment_runs DROP COLUMN IF EXISTS registered_by;
ALTER TABLE experiment_proposals ADD COLUMN IF NOT EXISTS revision_id UUID NULL;
ALTER TABLE experiment_proposals DROP COLUMN IF EXISTS design_id;
DROP TRIGGER IF EXISTS experiment_results_insert_only ON experiment_results;
DROP TABLE IF EXISTS experiment_designs;
DROP TABLE IF EXISTS experiments;
DROP FUNCTION IF EXISTS experiment_refuse_update();
"#,
            )
            .await?;
        Ok(())
    }
}
