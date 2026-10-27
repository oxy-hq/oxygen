//! An experiment's design stops being a list of insert-only versions and becomes
//! one row: a draft edited in place until Register freezes it. `frozen_at` is
//! the server's registration instant — the pre-registration proof — and from
//! then on the database refuses to change or delete the row. A proposal keeps
//! the `design_hash` it priced, so an edit since then reads as stale.
//!
//! Existing data: a design a run is pinned to is kept and frozen at the run's
//! `registered_at`. Every other experiment keeps its latest version as the
//! draft and drops the rest; a proposal that priced a dropped version is
//! repointed at the kept row and keeps the hash it priced, so it shows stale.
//!
//! Revert-safe in the sense `internal-docs/revert-safe-migrations.md` asks:
//! idempotent (the data step is guarded on `version`, which it drops, and runs
//! as dynamic SQL so a re-run still parses). The contraction is declared in
//! `migration_rollback_safety.rs`: `version`, `parent_id` and the per-experiment
//! key were created by `m20261014_000001_experiments_as_records`, which has
//! never reached a deploy and ships to main together with this one.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const EXPAND_SQL: &str = r#"
ALTER TABLE experiment_designs ADD COLUMN IF NOT EXISTS frozen_at TIMESTAMPTZ NULL;
ALTER TABLE experiment_designs ADD COLUMN IF NOT EXISTS updated_by UUID NULL;
ALTER TABLE experiment_designs ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NULL;
ALTER TABLE experiment_proposals ADD COLUMN IF NOT EXISTS design_hash TEXT NULL;
-- The insert-only trigger refuses the backfill below; the frozen-only one
-- replaces it at the end.
DROP TRIGGER IF EXISTS experiment_designs_insert_only ON experiment_designs;
"#;

/// Keep one design per experiment: the run's, else the highest version.
const COLLAPSE_SQL: &str = r#"
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM information_schema.columns
               WHERE table_schema = current_schema()
                 AND table_name = 'experiment_designs' AND column_name = 'version') THEN
        EXECUTE $q$
            UPDATE experiment_proposals p SET design_hash = d.content_hash
            FROM experiment_designs d
            WHERE p.design_id = d.id AND p.design_hash IS NULL
        $q$;
        EXECUTE $q$
            CREATE TEMP TABLE experiment_designs_kept ON COMMIT DROP AS
            SELECT DISTINCT ON (d.experiment_id) d.experiment_id, d.id
            FROM experiment_designs d
            LEFT JOIN experiment_runs r ON r.design_id = d.id
            ORDER BY d.experiment_id, (r.id IS NOT NULL) DESC, d.version DESC
        $q$;
        EXECUTE $q$
            UPDATE experiment_proposals p SET design_id = k.id
            FROM experiment_designs d
            JOIN experiment_designs_kept k ON k.experiment_id = d.experiment_id
            WHERE p.design_id = d.id AND d.id <> k.id
        $q$;
        EXECUTE $q$ UPDATE experiment_designs SET parent_id = NULL $q$;
        EXECUTE $q$
            DELETE FROM experiment_designs d
            WHERE NOT EXISTS (SELECT 1 FROM experiment_designs_kept k WHERE k.id = d.id)
        $q$;
        EXECUTE $q$ DROP TABLE experiment_designs_kept $q$;
    END IF;
END
$$;
"#;

const CONTRACT_SQL: &str = r#"
UPDATE experiment_designs d SET frozen_at = r.registered_at
FROM experiment_runs r
WHERE r.design_id = d.id AND d.frozen_at IS NULL;
UPDATE experiment_designs SET updated_by = created_by, updated_at = created_at
WHERE updated_at IS NULL;
ALTER TABLE experiment_designs ALTER COLUMN updated_by SET NOT NULL;
ALTER TABLE experiment_designs ALTER COLUMN updated_at SET DEFAULT now();
ALTER TABLE experiment_designs ALTER COLUMN updated_at SET NOT NULL;

ALTER TABLE experiment_designs DROP CONSTRAINT IF EXISTS experiment_designs_experiment_version_key;
ALTER TABLE experiment_designs DROP COLUMN IF EXISTS parent_id;
ALTER TABLE experiment_designs DROP COLUMN IF EXISTS version;
-- One design per experiment.
CREATE UNIQUE INDEX IF NOT EXISTS uq_experiment_designs_experiment
    ON experiment_designs (experiment_id);

-- A frozen design is evidence a run is pinned to: an UPDATE or a DELETE of one
-- is an error whoever issues it. A draft (frozen_at NULL) is edited in place,
-- and freezing it is that last edit.
CREATE OR REPLACE FUNCTION experiment_design_refuse_frozen() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.frozen_at IS NOT NULL THEN
        RAISE EXCEPTION 'experiment design % is frozen: it was registered at %, and a frozen design never changes',
            OLD.id, OLD.frozen_at
            USING ERRCODE = 'restrict_violation';
    END IF;
    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END
$$;

DROP TRIGGER IF EXISTS experiment_designs_frozen ON experiment_designs;
CREATE TRIGGER experiment_designs_frozen BEFORE UPDATE OR DELETE ON experiment_designs
    FOR EACH ROW EXECUTE FUNCTION experiment_design_refuse_frozen();
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(EXPAND_SQL).await?;
        db.execute_unprepared(COLLAPSE_SQL).await?;
        db.execute_unprepared(CONTRACT_SQL).await?;
        Ok(())
    }

    /// Schema only: every kept design comes back as version 1 with no parent,
    /// and the dropped versions are gone.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
DROP TRIGGER IF EXISTS experiment_designs_frozen ON experiment_designs;
DROP FUNCTION IF EXISTS experiment_design_refuse_frozen();
DROP INDEX IF EXISTS uq_experiment_designs_experiment;
ALTER TABLE experiment_designs ADD COLUMN IF NOT EXISTS version INTEGER NOT NULL DEFAULT 1;
ALTER TABLE experiment_designs ADD COLUMN IF NOT EXISTS parent_id UUID NULL REFERENCES experiment_designs(id);
ALTER TABLE experiment_designs DROP CONSTRAINT IF EXISTS experiment_designs_experiment_version_key;
ALTER TABLE experiment_designs ADD CONSTRAINT experiment_designs_experiment_version_key UNIQUE (experiment_id, version);
ALTER TABLE experiment_designs DROP COLUMN IF EXISTS frozen_at;
ALTER TABLE experiment_designs DROP COLUMN IF EXISTS updated_by;
ALTER TABLE experiment_designs DROP COLUMN IF EXISTS updated_at;
ALTER TABLE experiment_proposals DROP COLUMN IF EXISTS design_hash;
DROP TRIGGER IF EXISTS experiment_designs_insert_only ON experiment_designs;
CREATE TRIGGER experiment_designs_insert_only BEFORE UPDATE ON experiment_designs
    FOR EACH ROW EXECUTE FUNCTION experiment_refuse_update();
"#,
            )
            .await?;
        Ok(())
    }
}
