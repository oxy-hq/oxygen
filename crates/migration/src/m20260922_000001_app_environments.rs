//! Named environments for custom apps: see
//! `internal-docs/2026-09-10-custom-app-environments-design.md` §3.
//!
//! Phase 1a: the rows mirror `apps.draft_build_id` (staging) and
//! `apps.published_build_id` (production). Those columns stay the source readers
//! use until Phase 1b, and are dropped in Phase 1c. Dropping them is forward-only:
//! never revert a PR that deletes an applied migration file.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS app_environments (
    app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    kind TEXT NOT NULL,
    owner_user_id UUID REFERENCES users(id) ON DELETE CASCADE,
    -- Default NO ACTION, deliberately not RESTRICT. NO ACTION is checked at the end
    -- of the statement, so deleting an app (which cascades to both app_builds and
    -- this table) succeeds, while deleting a build an environment still serves
    -- fails. RESTRICT fires mid-cascade and would break app deletion.
    build_id UUID REFERENCES app_builds(id),
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (app_id, name),
    -- Text + CHECK rather than a PG enum, matching apps.visibility.
    CONSTRAINT app_environments_kind_check
        CHECK (kind IN ('production', 'staging', 'dev')),
    -- Mirrors oxy_app_core::custom_app_environment::is_valid_dev_handle. A DB test
    -- (database_and_rust_agree_on_environment_names) pins the two together.
    -- The position() clause is load-bearing, not redundant: the regex alone accepts
    -- 'dev-a--b', because [a-z0-9-]{0,10} swallows the '--'. The parity test covers
    -- exactly that name.
    CONSTRAINT app_environments_name_matches_kind CHECK (
        (kind = 'production' AND name = 'production' AND owner_user_id IS NULL)
        OR (kind = 'staging' AND name = 'staging' AND owner_user_id IS NULL)
        OR (
            kind = 'dev'
            AND owner_user_id IS NOT NULL
            AND name ~ '^dev-[a-z0-9]([a-z0-9-]{0,10}[a-z0-9])?$'
            AND position('--' in substr(name, 5)) = 0
        )
    )
);

CREATE TABLE IF NOT EXISTS app_environment_events (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    -- SET NULL: gc_builds reaps old build rows, and history must survive that.
    build_id UUID REFERENCES app_builds(id) ON DELETE SET NULL,
    action TEXT NOT NULL,
    actor UUID REFERENCES users(id) ON DELETE SET NULL,
    at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT app_environment_events_action_check CHECK (
        action IN ('publish', 'promote', 'rollback', 'unpublish', 'reset', 'backfill')
    )
);
CREATE INDEX IF NOT EXISTS idx_app_environment_events_app_env_at
    ON app_environment_events (app_id, environment, at DESC);
CREATE INDEX IF NOT EXISTS idx_app_environment_events_build
    ON app_environment_events (build_id);
-- Postgres does not index the referencing side of a foreign key. Without this, every
-- DELETE FROM app_builds (gc_builds after each publish, app deletion) runs the
-- build_id NO ACTION check as a sequential scan of app_environments.
CREATE INDEX IF NOT EXISTS idx_app_environments_build
    ON app_environments (build_id);
"#;

/// Seeds environments from today's pointers. Public so a DB test can run it against
/// seeded rows. The template database has already migrated, so the test can't
/// observe the migration's own run.
///
/// Idempotent: rows use ON CONFLICT DO NOTHING, events use NOT EXISTS.
pub const BACKFILL_SQL: &str = r#"
-- production <- published_build_id, only when that build row still exists. The
-- pointer columns never had an FK, and a dangling one would fail the new FK and
-- abort every pending migration at startup.
INSERT INTO app_environments (app_id, name, kind, build_id)
SELECT a.id, 'production', 'production', p.id
FROM apps a
JOIN app_builds p ON p.id = a.published_build_id
ON CONFLICT (app_id, name) DO NOTHING;

-- staging <- draft_build_id, falling back to the published build, so an app that
-- is already live never starts with an empty staging.
INSERT INTO app_environments (app_id, name, kind, build_id)
SELECT a.id, 'staging', 'staging', COALESCE(d.id, p.id)
FROM apps a
LEFT JOIN app_builds d ON d.id = a.draft_build_id
LEFT JOIN app_builds p ON p.id = a.published_build_id
WHERE COALESCE(d.id, p.id) IS NOT NULL
ON CONFLICT (app_id, name) DO NOTHING;

-- Every build either pointer names counts as having been served by staging (spec
-- §5.2), so today's live builds stay promotable and roll-back-able.
INSERT INTO app_environment_events (app_id, environment, build_id, action)
SELECT DISTINCT a.id, 'staging', b.id, 'backfill'
FROM apps a
JOIN app_builds b ON b.id = a.draft_build_id OR b.id = a.published_build_id
WHERE NOT EXISTS (
    SELECT 1 FROM app_environment_events e
    WHERE e.app_id = a.id AND e.environment = 'staging'
      AND e.build_id = b.id AND e.action = 'backfill'
);

INSERT INTO app_environment_events (app_id, environment, build_id, action)
SELECT a.id, 'production', b.id, 'backfill'
FROM apps a
JOIN app_builds b ON b.id = a.published_build_id
WHERE NOT EXISTS (
    SELECT 1 FROM app_environment_events e
    WHERE e.app_id = a.id AND e.environment = 'production'
      AND e.build_id = b.id AND e.action = 'backfill'
);
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(UP_SQL).await?;
        db.execute_unprepared(BACKFILL_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            DROP TABLE IF EXISTS app_environment_events;
            DROP TABLE IF EXISTS app_environments;
        "#,
            )
            .await?;
        Ok(())
    }
}
