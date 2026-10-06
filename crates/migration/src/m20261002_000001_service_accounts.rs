//! API tokens, Phase 3: org-owned service accounts. Design:
//! `internal-docs/2026-09-30-api-tokens-design.md` §3.3, §7.
//!
//! Expand-only: one new table. A binary one release back reads nothing here,
//! and refuses every `service_account` token by kind (§4.7), so a revert
//! fails them closed.
//!
//! A service account is a `users` row with a NULL email and no login path,
//! plus this row naming its org and its standing there. It is deliberately
//! **not** an `org_members` row — membership is enumerated everywhere (seats,
//! member lists, invitations, app audiences, teams) — so its standing is this
//! row and nothing else.
//!
//! - `org_role` is `member | admin`, and the CHECK is the invariant: a service
//!   account is never an Owner, so nothing that picks "an owner of the org"
//!   can ever pick one.
//! - `name` is a slug, unique per org.
//! - Deleting the org or the `users` row deletes the account; its tokens then
//!   resolve to no account and are refused.
//! - `disabled_at` stops every token of the account at once, and is undone by
//!   clearing it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS service_accounts (
    user_id UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    org_role TEXT NOT NULL CHECK (org_role IN ('member', 'admin')),
    name TEXT NOT NULL,
    description TEXT,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    disabled_at TIMESTAMPTZ
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_service_accounts_org_name
    ON service_accounts (org_id, name);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS service_accounts;
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
