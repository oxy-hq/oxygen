use sea_orm_migration::prelude::*;

/// The platform-schema version each custom-app environment's own OLTP branch
/// is known to carry, beside `oltp_branches.platform_schema_version` (the
/// org's staging branch's) and `oltp_tenants.platform_schema_version`.
///
/// A sandbox on an OLTP branch of its own publishes events into that
/// branch's outbox (app events design §6.7), so the branch needs the outbox's
/// current platform steps (step 6, the provenance MAC, among them). It gets
/// them lazily, on the outbox's first grant or claim there, from this
/// version (`oxy_oltp::events`), exactly as the staging branch does.
///
/// `DEFAULT 0` for a row cut before this column, and a cut or reset records 0
/// again: each is a fresh copy of production at a version nobody read, so
/// its first touch runs every step, each idempotent (`platform.rs` rule 2).
/// Additive, so the previous binary reads and writes the table as before.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE oltp_env_branches \
                 ADD COLUMN IF NOT EXISTS platform_schema_version INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE oltp_env_branches DROP COLUMN IF EXISTS platform_schema_version",
            )
            .await?;
        Ok(())
    }
}
