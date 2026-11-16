//! Revoke every legacy API key **nobody has used**.
//!
//! The code that honoured `oxy_<32 hex>` is gone, so these keys already
//! authenticate nothing. This writes that down: a key whose row still reads
//! *active* is a key someone will keep looking at and keep trusting, and the
//! inventory, the audit trail and the org's own list of credentials should all
//! agree with what the server actually does.
//!
//! **A key with a `last_used_at` is left alone, deliberately.** Eleven of the
//! fifteen keys that existed on 2026-11-16 had never been used; four were live,
//! and two of those belong to a customer and hold their credential outside this
//! repository (`internal-docs/api-token-migration.md` names all four). Revoking
//! a live key here would make reverting the server image useless as a way out:
//! the previous binary refuses a legacy row once `api_keys.is_active = false`,
//! so a rollback would restore the code path and not the credential, and
//! getting those integrations back would mean hand-editing rows in two tables.
//! Leaving them active costs nothing — this release's code refuses them either
//! way — and keeps a revert a real escape hatch until each owner has swapped.
//!
//! So the sequence is: this release removes the auth path, each owner swaps
//! their key for a token, and **a later migration ends the four live rows** once
//! `last_used_at` shows the replacement is carrying the traffic. That migration
//! is the only thing still outstanding; it is a one-line change of the `WHERE`
//! below.
//!
//! "Used" means either row: a key and its `api_tokens` mirror could record the
//! use on different sides of the lift, so each statement checks the other table
//! before it writes.
//!
//! **Data only.** Nothing is dropped. `api_keys` keeps its rows and columns and
//! `api_tokens` keeps `legacy_api_key_id`, because reverting the server image
//! tolerates newer ledger rows but not a missing column — a binary one release
//! back must still be able to read both tables.
//!
//! `revoke_reason = 'legacy_removed'` so the reason survives the rollout and
//! an operator can tell these apart from a key its owner ended.
//!
//! Idempotent: both statements skip a row that is already revoked, so a re-run
//! and a re-apply after a revert are no-ops.

use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"
UPDATE api_tokens t
   SET revoked_at = now(),
       revoke_reason = 'legacy_removed'
 WHERE t.revoked_at IS NULL
   AND (t.kind = 'legacy_key' OR t.legacy_api_key_id IS NOT NULL)
   AND t.last_used_at IS NULL
   AND NOT EXISTS (
         SELECT 1
           FROM api_keys k
          WHERE k.id = t.legacy_api_key_id
            AND k.last_used_at IS NOT NULL);

UPDATE api_keys k
   SET is_active = false,
       updated_at = now()
 WHERE k.is_active
   AND k.last_used_at IS NULL
   AND NOT EXISTS (
         SELECT 1
           FROM api_tokens t
          WHERE t.legacy_api_key_id = k.id
            AND t.last_used_at IS NOT NULL);
"#;

/// Down is deliberately a no-op.
///
/// Un-revoking would hand back credentials that no binary in this lineage
/// accepts, and it could not tell a key this migration ended from one its owner
/// revoked the day before — the `is_active` flag does not record who set it.
/// Rolling the schema back is safe; bringing the keys back is a decision for a
/// person, one key at a time.
const DOWN_SQL: &str = "SELECT 1";

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
