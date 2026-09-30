use sea_orm_migration::prelude::*;

/// `app_builds.semantic_revision_id` — the staging pin.
///
/// A custom-app draft build published with `oxyc publish --semantic-branch`
/// records the compiled revision (kind `staging`, or a ready `main` of the
/// same SHA) its staging requests read. Build metadata on an existing
/// control-plane table, not org data.
///
/// **No foreign key, deliberately.** Adding one to an existing table is a
/// contraction under `migration_rollback_safety` (one deploy back must still
/// write). Nothing needs it: retention
/// (`compile_maintenance::prune_old_revisions`) skips pinned revisions, and
/// `custom_apps_staging_pin::pinned_revision_for` joins `revisions` and reads a
/// dangling id as "no pin" — the promoted revision, today's behaviour.
///
/// A plain `CREATE INDEX`, not `CONCURRENTLY` as later index adds are asked to
/// be (`m20260606_000002`): `app_builds` is a small control-plane table and the
/// column is new and all-NULL, so the `SHARE` lock is momentary.
///
/// The partial index serves retention's `NOT EXISTS (… WHERE
/// semantic_revision_id = r.revision_id)` probe; almost every build is
/// unpinned, so it stays tiny.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "ALTER TABLE app_builds \
             ADD COLUMN IF NOT EXISTS semantic_revision_id uuid NULL",
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_app_builds_semantic_revision_id \
             ON app_builds (semantic_revision_id) \
             WHERE semantic_revision_id IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("DROP INDEX IF EXISTS idx_app_builds_semantic_revision_id")
            .await?;
        db.execute_unprepared("ALTER TABLE app_builds DROP COLUMN IF EXISTS semantic_revision_id")
            .await?;
        Ok(())
    }
}
