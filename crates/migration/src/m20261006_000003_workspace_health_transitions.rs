use sea_orm_migration::prelude::*;

/// Keep each change of a workspace's health status, not only the latest.
///
/// # Why
///
/// `workspace_health_state` is one row per workspace, overwritten by every
/// evaluation. It can say a workspace is unhealthy and since when; it cannot
/// say that the same workspace was unhealthy for three hours on Tuesday and
/// recovered, or that this is the fourth time this month. That history existed
/// only as Slack messages.
///
/// `workspace_health_transitions` is append-only: one row each time an
/// evaluation finds a status different from the last one, carrying the status
/// it left, the status it entered, and which dimensions were failing at that
/// moment. Dimension names and statuses only — the reason strings, which can
/// quote a connector's error text, stay on the state row and are not kept
/// here. `workspace_id` is loose, as it is on the state table. The evaluation
/// that writes a row also deletes that workspace's rows older than ninety
/// days.
///
/// # Backfill
///
/// One row per existing state row, dated at its `changed_at`: the start of the
/// state each workspace is in now. Without it a workspace that has been
/// unhealthy since last week would show an empty history until it next
/// changed. `from_status` is NULL for these — what came before was not
/// recorded. The state table has one row per opted-in workspace, so this is a
/// few hundred rows at most. Idempotent: a workspace that already has a
/// transition is skipped.
///
/// # Locking
///
/// A new table and an index on it. The backfill reads `workspace_health_state`
/// once and takes no lock that blocks an evaluation.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// The start of each workspace's current state, as a first transition.
///
/// A dimension is carried only when it is failing, in the `{dimension,
/// status}` shape the evaluator writes. A payload from before dimensions were
/// stored, or with none failing, yields an empty list.
pub const BACKFILL_SQL: &str = "\
INSERT INTO workspace_health_transitions (workspace_id, at, from_status, to_status, failures) \
SELECT s.workspace_id, s.changed_at, NULL, s.status, \
       COALESCE((SELECT jsonb_agg(jsonb_build_object('dimension', d->>'dimension', \
                                                      'status', d->>'status') \
                                  ORDER BY d->>'dimension') \
                 FROM jsonb_array_elements( \
                        CASE WHEN jsonb_typeof(s.payload->'dimensions') = 'array' \
                             THEN s.payload->'dimensions' ELSE '[]'::jsonb END) d \
                 WHERE d->>'status' IN ('degraded', 'unhealthy')), '[]'::jsonb) \
FROM workspace_health_state s \
WHERE NOT EXISTS (SELECT 1 FROM workspace_health_transitions t \
                  WHERE t.workspace_id = s.workspace_id)";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            "CREATE TABLE IF NOT EXISTS workspace_health_transitions (\
               id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, \
               workspace_id UUID NOT NULL, \
               at TIMESTAMPTZ NOT NULL, \
               from_status TEXT, \
               to_status TEXT NOT NULL, \
               failures JSONB NOT NULL DEFAULT '[]'::jsonb\
             )",
        )
        .await?;
        // The history of one workspace, newest first, is the only read.
        db.execute_unprepared(
            "CREATE INDEX IF NOT EXISTS idx_workspace_health_transitions_workspace_at \
             ON workspace_health_transitions (workspace_id, at DESC)",
        )
        .await?;
        db.execute_unprepared(BACKFILL_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS workspace_health_transitions")
            .await?;
        Ok(())
    }
}
