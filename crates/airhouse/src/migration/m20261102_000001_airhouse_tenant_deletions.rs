use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

/// The Airhouse tenants of deleted workspaces that are still to be removed in
/// Airhouse.
///
/// A workspace delete cascades its `airhouse_tenants` row away, so it records
/// the tenant here in the same transaction, and a sweeper on the worker fleet
/// removes the tenant and then the row (`airhouse::workspace_delete`). There is
/// deliberately no foreign key to `workspaces`: the row exists because its
/// workspace is gone. A row that keeps failing stays, with `last_error`, as the
/// record of what is left to delete by hand.
#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
            CREATE TABLE IF NOT EXISTS airhouse_tenant_deletions (
                airhouse_tenant_id  TEXT PRIMARY KEY,
                workspace_id        UUID NOT NULL,
                service_account_id  TEXT,
                created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
                attempts            INTEGER NOT NULL DEFAULT 0,
                next_attempt_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
                last_error          TEXT
            );
            CREATE INDEX IF NOT EXISTS idx_airhouse_tenant_deletions_due
                ON airhouse_tenant_deletions (next_attempt_at);
        "#,
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS airhouse_tenant_deletions;")
            .await?;
        Ok(())
    }
}
