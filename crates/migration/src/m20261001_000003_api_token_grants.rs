//! API tokens, Phase 2: grants, the `oxyc login` code store, and the token an
//! assume-role session belongs to. Design:
//! `internal-docs/2026-09-30-api-tokens-design.md` §3.2, §6, §7.
//!
//! Expand-only: two new tables and one nullable column. A binary one release
//! back reads none of them, and inserts `admin_assume_sessions` rows with a
//! NULL `token_id` — "opened in a browser", which is what every session it
//! opens is.
//!
//! - `api_token_grants` — what a token with `all_access = false` covers. One
//!   row is `(org, one workspace | every workspace, role ceiling)`; an
//!   `app_publish` row (Phase 4) names an app instead. `kind` and
//!   `role_ceiling` carry no CHECK on purpose: a validator refuses a value it
//!   does not know (§4.7), so a later release can add one without this one ever
//!   honouring it. Every reference cascades, so deleting an org, workspace or
//!   app ends the grant — and a token left with no grant reaches nothing.
//! - `cli_auth_codes` — the single-use, five-minute code of the PKCE loopback
//!   login. Only its SHA-256 is stored, with the S256 challenge it must be
//!   redeemed against.
//! - `admin_assume_sessions.token_id` — the new-format token that opened the
//!   session; NULL for a browser session (and for a legacy key, which shares
//!   the browser's). **No foreign key**: `ON DELETE SET NULL` would turn a
//!   token's session into one every browser session inherits, and a cascade
//!   would delete audit history. A dangling id simply matches no credential.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS api_token_grants (
    id UUID PRIMARY KEY,
    token_id UUID NOT NULL REFERENCES api_tokens(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    workspace_id UUID REFERENCES workspaces(id) ON DELETE CASCADE,
    role_ceiling TEXT,
    app_id UUID REFERENCES apps(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    revoked_by UUID REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_api_token_grants_token_id
    ON api_token_grants (token_id);
CREATE INDEX IF NOT EXISTS idx_api_token_grants_org_id
    ON api_token_grants (org_id);
CREATE INDEX IF NOT EXISTS idx_api_token_grants_workspace_id
    ON api_token_grants (workspace_id) WHERE workspace_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS cli_auth_codes (
    code_hash BYTEA PRIMARY KEY,
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_challenge TEXT NOT NULL,
    hostname TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    consumed_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_cli_auth_codes_expires_at
    ON cli_auth_codes (expires_at);

ALTER TABLE admin_assume_sessions ADD COLUMN IF NOT EXISTS token_id UUID;
"#;

const DOWN_SQL: &str = r#"
ALTER TABLE admin_assume_sessions DROP COLUMN IF EXISTS token_id;
DROP TABLE IF EXISTS cli_auth_codes;
DROP TABLE IF EXISTS api_token_grants;
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
