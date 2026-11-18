use sea_orm_migration::prelude::*;

/// The platform-schema version each org staging branch is known to carry,
/// beside `oltp_tenants.platform_schema_version`.
///
/// A branch is a copy of production's database taken when it was cut, so it
/// holds the Oxy-owned objects production had then and nothing added since:
/// a branch cut before the events outbox (platform step 5) has no
/// `oxy_events`. The steps are applied to it lazily, on first touch, from this
/// version (`oxy_oltp::events`), exactly as a tenant is brought current from
/// its own.
///
/// `DEFAULT 0` for a row cut before this column: its version is unknown, so
/// its first touch runs every step, each idempotent (`platform.rs` rule 2). A
/// cut or reset records the tenant's version as read before the copy, a lower
/// bound of what the copy holds. Additive, so the previous binary reads and
/// writes the table as before (a row it inserts records 0).
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE oltp_branches \
                 ADD COLUMN IF NOT EXISTS platform_schema_version INTEGER NOT NULL DEFAULT 0",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE oltp_branches DROP COLUMN IF EXISTS platform_schema_version",
            )
            .await?;
        Ok(())
    }
}
