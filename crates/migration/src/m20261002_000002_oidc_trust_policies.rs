//! API tokens, Phase 4: trusted access. Design:
//! `internal-docs/2026-09-30-api-tokens-design.md` §3.4, §7.
//!
//! Expand-only: two new tables and two nullable columns. A binary one release
//! back reads none of it, and refuses every `ci` token by kind (§4.7), so a
//! revert fails them closed. Nothing here touches `app_publishers`,
//! `app_publish_tokens` or `oidc_used_jti`: the legacy trusted-publishing
//! exchange keeps reading and writing them exactly as before.
//!
//! - `oidc_trust_policies` — *a GitHub Actions run matching these claims may
//!   act as this service account*. It hangs off the account and goes with it:
//!   deleting the account (or its org) deletes its policies. The repository is
//!   named by GitHub's **numeric** ids; `repository` is display only.
//! - `oidc_trust_policy_grants` — what a matching run is granted. The same
//!   shape as `api_token_grants`, minus revocation: a policy's grants are
//!   replaced as a set, never revoked one by one.
//! - `api_tokens.trust_policy_id` / `oidc_claims` — on a `ci` token, the
//!   policy that minted it and the verified claims of the run. The policy link
//!   is a plain uuid, **not** a foreign key: a constraint added to the existing
//!   `api_tokens` table is a contraction the rollback-safety guard refuses.
//!   `trust_policy::delete` clears the link itself; a policy that goes with its
//!   account or org leaves a dangling id, which nothing joins on to authorize.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS oidc_trust_policies (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    service_account_id UUID NOT NULL REFERENCES service_accounts(user_id) ON DELETE CASCADE,
    provider TEXT NOT NULL DEFAULT 'github_actions',
    repository_owner_id BIGINT NOT NULL,
    repository_id BIGINT NOT NULL,
    repository TEXT NOT NULL,
    workflow_path TEXT NOT NULL,
    environment TEXT,
    ref_pattern TEXT,
    allow_self_hosted BOOLEAN NOT NULL DEFAULT false,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    disabled_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_oidc_trust_policies_repository
    ON oidc_trust_policies (repository_id);
CREATE INDEX IF NOT EXISTS idx_oidc_trust_policies_service_account
    ON oidc_trust_policies (service_account_id);

CREATE TABLE IF NOT EXISTS oidc_trust_policy_grants (
    id UUID PRIMARY KEY,
    policy_id UUID NOT NULL REFERENCES oidc_trust_policies(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    workspace_id UUID REFERENCES workspaces(id) ON DELETE CASCADE,
    role_ceiling TEXT,
    app_id UUID REFERENCES apps(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_oidc_trust_policy_grants_policy
    ON oidc_trust_policy_grants (policy_id);

ALTER TABLE api_tokens ADD COLUMN IF NOT EXISTS trust_policy_id UUID;
ALTER TABLE api_tokens ADD COLUMN IF NOT EXISTS oidc_claims JSONB;
CREATE INDEX IF NOT EXISTS idx_api_tokens_trust_policy
    ON api_tokens (trust_policy_id) WHERE trust_policy_id IS NOT NULL;
"#;

const DOWN_SQL: &str = r#"
DROP INDEX IF EXISTS idx_api_tokens_trust_policy;
ALTER TABLE api_tokens DROP COLUMN IF EXISTS oidc_claims;
ALTER TABLE api_tokens DROP COLUMN IF EXISTS trust_policy_id;
DROP TABLE IF EXISTS oidc_trust_policy_grants;
DROP TABLE IF EXISTS oidc_trust_policies;
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
