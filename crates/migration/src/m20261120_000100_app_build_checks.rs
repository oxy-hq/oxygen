//! `app_build_checks`: the promote checks of a custom-app build
//! (`entity::app_build_checks`), one row per build × environment × kind ×
//! attempt, and `app_builds.sdk_version`, the `@oxy-hq/sdk` a bundle was
//! built with, which `oxyc publish` now sends.
//!
//! Control plane, like `app_builds`: statuses, a one-line summary and capped
//! findings; no answer bodies, no page content. Deleted with the build.
//!
//! `runner` is who is running the attempt, the same value on every row of
//! it. Null while a started runner has not claimed it yet. A runner,
//! `{label, url, started_at}`, once one claims it (the GitHub Actions run the
//! worker dispatched, or a person running `oxyc checks run` by hand); every
//! claim writes its own `started_at`. Or `{not_started: "not_configured",
//! reason, at}`, written by the task on a deployment that starts no runner,
//! until a claim replaces it. That is how a reader tells "a runner is late",
//! "a runner started and stopped" and "nothing was ever going to pick this
//! up" apart.
//!
//! `notes` and the advisory flag are here from the first day so the advisory
//! notes of later phases (a rule's or a model's, on a check or as the build's
//! summary) need no second migration.
//!
//! Additive and revert-safe: a new table and a nullable column, which a binary
//! one release back never reads (its build inserts name the columns they set).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS app_build_checks (
    id UUID PRIMARY KEY,
    app_id UUID NOT NULL REFERENCES apps (id) ON DELETE CASCADE,
    build_id UUID NOT NULL REFERENCES app_builds (id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    kind TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    status TEXT NOT NULL,
    advisory BOOLEAN NOT NULL DEFAULT FALSE,
    summary TEXT,
    detail JSONB,
    notes JSONB NOT NULL DEFAULT '[]'::jsonb,
    run_id TEXT,
    runner JSONB,
    created_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    CONSTRAINT app_build_checks_one_per_attempt UNIQUE (build_id, environment, kind, attempt)
);
CREATE INDEX IF NOT EXISTS app_build_checks_app_idx ON app_build_checks (app_id, build_id);
ALTER TABLE app_builds ADD COLUMN IF NOT EXISTS sdk_version TEXT;
"#;

const DOWN_SQL: &str = r#"
ALTER TABLE app_builds DROP COLUMN IF EXISTS sdk_version;
DROP TABLE IF EXISTS app_build_checks;
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
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
