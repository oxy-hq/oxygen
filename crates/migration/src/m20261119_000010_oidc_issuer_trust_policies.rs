//! Trusted access from any OIDC issuer an org registers. Design:
//! `internal-docs/2026-09-30-api-tokens-design.md` §3.4, *Any OIDC issuer*.
//!
//! Expand-only: three new tables, nothing altered. A binary one release back
//! reads none of them; it answers such a token at the exchange as one it
//! cannot verify, and the `ci` tokens these policies minted are ordinary `ci`
//! rows it already knows how to authenticate and revoke.
//!
//! - `oidc_issuers` — a token issuer an org trusts, registered by its admin:
//!   the `iss` its tokens carry (stored without a trailing `/`, which a
//!   token's `iss` is compared without too), and where its signing keys come
//!   from — `key_source`:
//!   - `discovery`: the `jwks_uri` read from its discovery document at
//!     registration (from `discovery_url` when set, else the issuer itself);
//!   - `jwks_uri`: a key-set URL the admin gave;
//!   - `inline`: the key set itself, in `jwks`, for an issuer Oxygen cannot
//!     reach. Nothing is ever fetched for it.
//!
//!   `jti_single_use` burns each token's `jti` at the exchange. `preset` is
//!   which settings-form preset filled the row in (`aws`, `custom`):
//!   shown, never read by verification. An exchange verifies a token only
//!   against an issuer registered in the org of the service account it names;
//!   the token's own `iss` never chooses where keys come from.
//! - `oidc_issuer_trust_policies` — *a token from this issuer whose claims meet
//!   these conditions may act as this Oxygen service account*. `conditions`
//!   is a JSON list of `{"claim", "value"}`: a top-level scalar claim, and an
//!   exact value or a prefix ending in one `*`. It hangs off both the account
//!   and the issuer, and goes with either. A separate table from
//!   `oidc_trust_policies` because that table's GitHub columns are
//!   `NOT NULL`: making them nullable would make a binary one release back
//!   fail to read the GitHub rows the moment one other row existed.
//! - `oidc_issuer_trust_policy_grants` — what a matching token is granted. The
//!   same shape as `oidc_trust_policy_grants`.
//!
//! The `ci` tokens these policies mint carry the policy in
//! `api_tokens.trust_policy_id` — a plain uuid, so it names a row of either
//! policy table.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS oidc_issuers (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    issuer TEXT NOT NULL,
    key_source TEXT NOT NULL,
    discovery_url TEXT,
    jwks_uri TEXT,
    jwks JSONB,
    jti_single_use BOOLEAN NOT NULL DEFAULT true,
    preset TEXT,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT uq_oidc_issuers_org_issuer UNIQUE (org_id, issuer),
    CONSTRAINT ck_oidc_issuers_key_source CHECK (
        (key_source = 'inline' AND jwks IS NOT NULL AND jwks_uri IS NULL AND discovery_url IS NULL)
        OR (key_source = 'jwks_uri' AND jwks_uri IS NOT NULL AND jwks IS NULL AND discovery_url IS NULL)
        OR (key_source = 'discovery' AND jwks_uri IS NOT NULL AND jwks IS NULL)
    )
);

CREATE TABLE IF NOT EXISTS oidc_issuer_trust_policies (
    id UUID PRIMARY KEY,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    service_account_id UUID NOT NULL REFERENCES service_accounts(user_id) ON DELETE CASCADE,
    issuer_id UUID NOT NULL REFERENCES oidc_issuers(id) ON DELETE CASCADE,
    conditions JSONB NOT NULL,
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    disabled_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS idx_oidc_issuer_trust_policies_service_account
    ON oidc_issuer_trust_policies (service_account_id);
CREATE INDEX IF NOT EXISTS idx_oidc_issuer_trust_policies_issuer
    ON oidc_issuer_trust_policies (issuer_id);

CREATE TABLE IF NOT EXISTS oidc_issuer_trust_policy_grants (
    id UUID PRIMARY KEY,
    policy_id UUID NOT NULL REFERENCES oidc_issuer_trust_policies(id) ON DELETE CASCADE,
    kind TEXT NOT NULL,
    org_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    workspace_id UUID REFERENCES workspaces(id) ON DELETE CASCADE,
    role_ceiling TEXT,
    app_id UUID REFERENCES apps(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_oidc_issuer_trust_policy_grants_policy
    ON oidc_issuer_trust_policy_grants (policy_id);
"#;

const DOWN_SQL: &str = r#"
DROP TABLE IF EXISTS oidc_issuer_trust_policy_grants;
DROP TABLE IF EXISTS oidc_issuer_trust_policies;
DROP TABLE IF EXISTS oidc_issuers;
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
