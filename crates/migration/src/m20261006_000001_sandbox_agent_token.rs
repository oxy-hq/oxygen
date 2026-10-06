//! The sandbox agent token (`oxy_sbx_`). Design:
//! `internal-docs/2026-10-03-sandbox-agent-credential-design.md` §7.1.
//!
//! Expand-only: three nullable columns and one index, no new table and no
//! constraint. A binary one release back reads none of them, writes each as
//! NULL, and refuses every `sandbox_agent` token by kind and every
//! `app_sandbox` grant by kind (API-tokens design §4.7), so a revert fails the
//! new credential closed.
//!
//! Neither `kind` column changes: `api_tokens.kind` and
//! `api_token_grants.kind` carry no CHECK on purpose, and the validator is what
//! learns the new values.
//!
//! - `app_environments.created_by_token_id` — the sandbox agent token that
//!   created a sandbox, which is what "its own sandbox" means for that token.
//!   NULL for every environment a person created. A plain uuid, **not** a
//!   foreign key: the design asked for `REFERENCES api_tokens(id) ON DELETE SET
//!   NULL`, and a constraint added to the existing `app_environments` table is
//!   a contraction the rollback-safety guard refuses — the same reason
//!   `api_tokens.trust_policy_id` is one. A token row that is gone (its minter's
//!   `users` row was deleted, which cascades) leaves a dangling id. No live
//!   token can ever match it, so the sandbox has no token owner and falls back
//!   to idle expiry, which is what `SET NULL` would have given.
//! - `app_function_invocations.credential_token_id` — the token a `/fn` call
//!   authenticated with. No foreign key by design, as
//!   `admin_assume_sessions.token_id`: `SET NULL` would erase the attribution
//!   when the minter's row goes, and a dangling id matches no credential.
//! - `cli_auth_codes.mint` — what a PKCE code was authorized to mint when it is
//!   not an `oxyc login`: the kind, the app ids, the lifetime and the name.
//!   NULL for a login code. Such a code is stored under a different hash than a
//!   login code's (`oxy_auth::token::cli_login`), so a binary one release back,
//!   which would mint an all-access login token for any code it can find, finds
//!   nothing.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
ALTER TABLE app_environments ADD COLUMN IF NOT EXISTS created_by_token_id UUID;
CREATE INDEX IF NOT EXISTS idx_app_environments_created_by_token
    ON app_environments (created_by_token_id) WHERE created_by_token_id IS NOT NULL;

ALTER TABLE app_function_invocations ADD COLUMN IF NOT EXISTS credential_token_id UUID;

ALTER TABLE cli_auth_codes ADD COLUMN IF NOT EXISTS mint JSONB;
"#;

const DOWN_SQL: &str = r#"
ALTER TABLE cli_auth_codes DROP COLUMN IF EXISTS mint;
ALTER TABLE app_function_invocations DROP COLUMN IF EXISTS credential_token_id;
DROP INDEX IF EXISTS idx_app_environments_created_by_token;
ALTER TABLE app_environments DROP COLUMN IF EXISTS created_by_token_id;
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
