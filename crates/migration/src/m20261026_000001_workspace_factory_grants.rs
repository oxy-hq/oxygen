//! Per-workspace Factory grants: which members an Oxy operator let into the Factory
//! (the IDE and back office) of one workspace. Plan decision 5,
//! `internal-docs/apps-first-product-plan.md` W2.
//!
//! Expand-only: one new table. A binary one release back never reads it, so a revert
//! simply stops looking at the grants — and enforcement is behind the
//! `factory_access_enforcement` feature flag, off by default, so nothing a revert
//! leaves behind refuses anyone.
//!
//! The row is the whole grant. `(workspace_id, user_id)` is the key, so granting twice
//! is one row. Deleting the workspace or the user deletes it; deleting the granting
//! operator keeps the grant and forgets who made it (`granted_by` is audit, never
//! authority). Membership of the workspace's org is NOT stored here: the authz ring
//! ANDs the grant with it, so a removed member's row opens nothing.
//!
//! The index is on `user_id`, because the loader reads "every workspace this user was
//! granted" once per request.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS workspace_factory_grants (
    workspace_id UUID NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    granted_by UUID REFERENCES users(id) ON DELETE SET NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, user_id)
);

CREATE INDEX IF NOT EXISTS idx_workspace_factory_grants_user
    ON workspace_factory_grants (user_id);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS workspace_factory_grants;
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
