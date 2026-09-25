use sea_orm_migration::prelude::*;

/// What the deploy preflight has already reported, so a release is blocked only
/// by what it changes.
///
/// `custom_apps_functions::preflight` runs in `oxy migrate` and judges every
/// live function against the new binary's host rules. A refusal the current
/// binary already enforces is not the release's doing — blocking on it would
/// hold every release until one app is fixed. Each rollout the preflight lets
/// through records the refusals it saw here; the next one blocks only on a
/// refusal absent from this table. A blocked rollout records nothing, so its
/// retry blocks again.
///
/// Keyed on the rule's stable name and the database (empty for a rule about
/// the whole manifest), not on the host's message: rewording a refusal must
/// not make it new. `reason` is the message as first seen, for people.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE TABLE IF NOT EXISTS app_preflight_refusals (\
                   app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE, \
                   function_name TEXT NOT NULL, \
                   rule TEXT NOT NULL, \
                   database TEXT NOT NULL DEFAULT '', \
                   reason TEXT NOT NULL, \
                   first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(), \
                   first_seen_release TEXT NOT NULL, \
                   PRIMARY KEY (app_id, function_name, rule, database)\
                 )",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS app_preflight_refusals")
            .await?;
        Ok(())
    }
}
