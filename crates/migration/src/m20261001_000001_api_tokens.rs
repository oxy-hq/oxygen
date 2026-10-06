//! `api_tokens`: every API credential, hashed at rest. Design:
//! `internal-docs/2026-09-30-api-tokens-design.md` §7, Release N (expand).
//!
//! Expand-only. It creates one table and copies `api_keys` into it; it changes
//! nothing an older binary reads, so a revert to the release before this one
//! keeps validating every key from `api_keys` exactly as it does today.
//!
//! - `token_hash = sha256(api_keys.key_hash)`. `key_hash` holds the plaintext
//!   key (a misnomer from day one), so this is the SHA-256 of the key itself —
//!   the same bytes `oxy_auth::token::hash_token` produces from the presented
//!   key.
//! - A backfilled row takes the `api_keys` id as its own. The id the API Keys
//!   UI already holds is then the token id every audit row names.
//! - `ON CONFLICT DO NOTHING`, so re-running is a no-op and a duplicate
//!   plaintext (the old index was never unique) keeps its first row.
//! - All three reach flags are true: a legacy key keeps everything it reaches
//!   today (§3.5).
//!
//! `kind` and `source` carry no CHECK on purpose. A validator refuses a kind it
//! does not know (§4.7), so a later release can add one without a migration
//! here and without this release ever honouring it.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS api_tokens (
    id UUID PRIMARY KEY,
    kind TEXT NOT NULL,
    principal_user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    display_prefix TEXT NOT NULL,
    last_four TEXT NOT NULL,
    token_hash BYTEA NOT NULL,
    -- Default false: a row written without them fails closed, because a
    -- validator refuses any restriction it cannot enforce.
    all_access BOOLEAN NOT NULL DEFAULT false,
    platform BOOLEAN NOT NULL DEFAULT false,
    partner BOOLEAN NOT NULL DEFAULT false,
    expires_at TIMESTAMPTZ,
    last_used_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_by UUID REFERENCES users(id) ON DELETE SET NULL,
    revoked_at TIMESTAMPTZ,
    revoked_by UUID REFERENCES users(id) ON DELETE SET NULL,
    revoke_reason TEXT,
    source TEXT NOT NULL,
    -- CASCADE: deleting the api_keys row (a workspace or user deletion cascades
    -- there today) ends the key, exactly as it does now.
    legacy_api_key_id UUID REFERENCES api_keys(id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_tokens_token_hash
    ON api_tokens (token_hash);
CREATE UNIQUE INDEX IF NOT EXISTS idx_api_tokens_legacy_api_key_id
    ON api_tokens (legacy_api_key_id);
CREATE INDEX IF NOT EXISTS idx_api_tokens_principal_user_id
    ON api_tokens (principal_user_id);
"#;

/// Copies every `api_keys` row into `api_tokens`. Public so a DB test can run it
/// against seeded rows; the template database has already migrated, so a test
/// cannot observe the migration's own run.
///
/// The column expressions are mirrored by `oxy_auth::token::store`'s lazy insert
/// (which covers a key an older pod mints mid-rollout). Change them together.
/// This copy is deliberate: a migration is a frozen snapshot and imports no runtime code.
pub const BACKFILL_SQL: &str = r#"
INSERT INTO api_tokens (
    id, kind, principal_user_id, name, display_prefix, last_four, token_hash,
    all_access, platform, partner, expires_at, last_used_at, created_at,
    created_by, revoked_at, source, legacy_api_key_id
)
SELECT
    k.id, 'legacy_key', k.user_id, k.name,
    left(k.key_hash, 4),
    CASE WHEN length(k.key_hash) > 8 THEN right(k.key_hash, 4) ELSE '' END,
    sha256(convert_to(k.key_hash, 'UTF8')),
    true, true, true,
    k.expires_at, k.last_used_at, k.created_at,
    k.user_id,
    CASE WHEN k.is_active THEN NULL ELSE k.updated_at END,
    'legacy_backfill', k.id
FROM api_keys k
ON CONFLICT DO NOTHING;
"#;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(UP_SQL).await?;
        db.execute_unprepared(BACKFILL_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE IF EXISTS api_tokens;")
            .await?;
        Ok(())
    }
}
