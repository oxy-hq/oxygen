//! An app's own platform access (`internal-docs/proposals/app-platform-access.md`):
//! the actions an org admin approved for a custom app, which its functions use
//! through host ops, as the app's service account, with no stored key.
//!
//! Expand-only: two new tables, nothing changed on an existing one. A binary one
//! release back reads neither, and refuses the `app` kind on `api_tokens` by kind
//! (API-tokens design §4.7), so a revert fails an app's access closed.
//!
//! - `app_service_accounts` — the one service account an app acts as. Its own
//!   table rather than a column on `service_accounts`, because a unique index
//!   added to an existing table is a contraction the rollback-safety guard
//!   refuses. One row per app; the account outlives a revoked approval, so a
//!   later approval reuses it and the audit trail keeps one actor.
//! - `app_platform_grants` — one row per approved action, under the app's
//!   secretless `api_tokens` row (kind `app`). The token row is what the org's
//!   API-access inventory lists, what audit rows stamp, and what a revoke ends;
//!   these rows say which actions it holds. Its own table rather than a column
//!   on `api_token_grants`, so no existing grant reader learns a new shape.
//!   Approvals are the org's and the app's, never a workspace's. `target` names
//!   what one operation may act on inside the org when the action has such a
//!   thing (a pipeline's landing zone for an upload); NULL for the whole action.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS app_service_accounts (
    app_id UUID PRIMARY KEY REFERENCES apps(id) ON DELETE CASCADE,
    service_account_user_id UUID NOT NULL UNIQUE
        REFERENCES service_accounts(user_id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS app_platform_grants (
    id UUID PRIMARY KEY,
    token_id UUID NOT NULL REFERENCES api_tokens(id) ON DELETE CASCADE,
    app_id UUID NOT NULL REFERENCES apps(id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    action TEXT NOT NULL,
    target TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID,
    revoked_at TIMESTAMPTZ,
    revoked_by UUID
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_app_platform_grants_live
    ON app_platform_grants (token_id, action, COALESCE(target, '')) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_app_platform_grants_app
    ON app_platform_grants (app_id);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS app_platform_grants;
DROP TABLE IF EXISTS app_service_accounts;
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
