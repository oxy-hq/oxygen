use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// A retrospective whose estimate was refused (a unit with no rows, a window too
/// sparsely covered) produced no result, so its design may be reopened: the
/// refused run is deleted and the design's `frozen_at` cleared, then edited and
/// analysed again instead of duplicated. The trigger still refuses every other
/// change to a frozen design: only clearing `frozen_at`, with the config and hash
/// untouched and no run left pointing at the design, passes.
const UP: &str = r#"
CREATE OR REPLACE FUNCTION experiment_design_refuse_frozen() RETURNS trigger
LANGUAGE plpgsql AS $$
BEGIN
    IF OLD.frozen_at IS NOT NULL THEN
        IF TG_OP = 'UPDATE'
           AND NEW.frozen_at IS NULL
           AND NEW.config = OLD.config
           AND NEW.content_hash = OLD.content_hash
           AND NOT EXISTS (SELECT 1 FROM experiment_runs r WHERE r.design_id = OLD.id) THEN
            RETURN NEW;
        END IF;
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
"#;

const DOWN: &str = r#"
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
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(DOWN).await?;
        Ok(())
    }
}
