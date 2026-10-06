//! API tokens, Phase 5: org token policy and token hygiene. Design:
//! `internal-docs/2026-09-30-api-tokens-design.md` §5, §8 Phase 5.
//!
//! Expand-only: one new table and three nullable columns. A binary one release
//! back reads none of them, so a revert leaves every token as that release
//! decides it — policies stop applying, which only ever gives reach back.
//!
//! - `org_token_policies` — one row per org that changed a default. No row
//!   means the defaults: no lifetime cap, all-access tokens allowed, and a
//!   trust policy must name an environment. The CHECK bounds the cap the API
//!   accepts (1–3650 days).
//! - `api_tokens.expiry_notified_at` — when the 7-day expiry notice went out
//!   for the token's current expiry. Extend clears it, so a new expiry gets a
//!   new notice.
//! - `api_tokens.expired_reason` — why hygiene ended a token by setting its
//!   `expires_at` (`unused`), so the row says it was not its owner. Extend
//!   clears it.
//! - `api_tokens.renewed_at` — when its owner last extended or regenerated
//!   it. The unused-token sweep counts that as activity, so a token revived by
//!   Extend is not expired again on the next pass.
//!
//! No constraint is added to `api_tokens`: one there would refuse rows the
//! previous release still writes (`migration_rollback_safety`).

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS org_token_policies (
    org_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    max_lifetime_days INTEGER CHECK (max_lifetime_days BETWEEN 1 AND 3650),
    allow_all_access_tokens BOOLEAN NOT NULL DEFAULT true,
    require_environment_on_trust_policies BOOLEAN NOT NULL DEFAULT true,
    updated_by UUID REFERENCES users(id) ON DELETE SET NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE api_tokens ADD COLUMN IF NOT EXISTS expiry_notified_at TIMESTAMPTZ;
ALTER TABLE api_tokens ADD COLUMN IF NOT EXISTS expired_reason TEXT;
ALTER TABLE api_tokens ADD COLUMN IF NOT EXISTS renewed_at TIMESTAMPTZ;
"#;

const DOWN_SQL: &str = r#"
ALTER TABLE api_tokens DROP COLUMN IF EXISTS renewed_at;
ALTER TABLE api_tokens DROP COLUMN IF EXISTS expired_reason;
ALTER TABLE api_tokens DROP COLUMN IF EXISTS expiry_notified_at;
DROP TABLE IF EXISTS org_token_policies;
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
