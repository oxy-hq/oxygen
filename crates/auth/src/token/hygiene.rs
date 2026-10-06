//! Token hygiene, the database half (API-tokens design §8 Phase 5): the
//! 7-day expiry notice and the unused-token sweep. The server's
//! `token_sweep` drives both from the global worker's tick; the mail and the
//! audit rows are written there.
//!
//! Three nullable `api_tokens` columns carry the state, read and written by
//! the SQL here only (the entity does not declare them):
//!
//! - `expiry_notified_at` — the notice for the current expiry went out.
//!   Claimed with a conditional `UPDATE` before the mail is sent, so any
//!   number of drivers send one notice per token per expiry;
//! - `expired_reason` — hygiene set this token's `expires_at` (`unused`);
//! - `renewed_at` — its owner last extended or regenerated it.
//!
//! [`reset_on_extend`] clears the first two and stamps the third. Every
//! extend calls it, so a new expiry gets a new notice and a revived token is
//! not swept again; a regenerate stamps the third alone ([`mark_renewed`]).
//!
//! **Legacy keys:** the notice covers them — it only informs. The sweep never
//! touches one (§3.5): its SQL selects new-format kinds only, and never a row
//! that mirrors `api_keys`.

use chrono::{DateTime, Duration, Utc};
use entity::api_tokens;
use entity::prelude::ApiTokens;
use oxy_shared::errors::OxyError;
use sea_orm::{ConnectionTrait, DatabaseBackend, EntityTrait, Statement};
use uuid::Uuid;

/// How long before `expires_at` the notice goes out.
pub const NOTICE_DAYS: i64 = 7;
/// A new-format token unused this long expires.
pub const UNUSED_DAYS: i64 = 365;
/// `api_tokens.expired_reason` for a token the unused sweep ended.
pub const UNUSED_REASON: &str = "unused";
/// The most tokens one pass handles of each job. The rest wait for the next
/// pass, ten minutes on, so a backlog never stalls the global driver.
pub const BATCH: i64 = 100;

/// Whether the expiry notice is due: the token expires within
/// [`NOTICE_DAYS`] and has not yet, no notice went out for this expiry, and
/// it already existed when the notice window opened — a token minted with a
/// shorter life than that was given it on purpose, and is not mailed.
pub fn notice_due(
    now: DateTime<Utc>,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    notified: bool,
) -> bool {
    let Some(expires_at) = expires_at else {
        return false;
    };
    let opens = expires_at - Duration::days(NOTICE_DAYS);
    !notified && expires_at > now && now >= opens && created_at <= opens
}

/// Whether a token is unused for [`UNUSED_DAYS`]: nothing — a use, its
/// creation, or its owner extending or regenerating it — happened since.
pub fn unused(
    now: DateTime<Utc>,
    created_at: DateTime<Utc>,
    last_used_at: Option<DateTime<Utc>>,
    renewed_at: Option<DateTime<Utc>>,
) -> bool {
    let latest = [Some(created_at), last_used_at, renewed_at]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(created_at);
    latest < now - Duration::days(UNUSED_DAYS)
}

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

fn stmt(sql: &str, values: Vec<sea_orm::Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// [`notice_due`] in SQL. Every kind that can be extended: a personal token, a
/// legacy key (whose `api_keys` row must still be active — a pod one release
/// back revokes there alone) and a service-account token. Never `ci`: it
/// lives 15 minutes and has no Extend.
const DUE_NOTICES_SQL: &str = r#"
SELECT t.* FROM api_tokens t
WHERE t.expiry_notified_at IS NULL
  AND t.revoked_at IS NULL
  AND t.kind IN ('personal', 'legacy_key', 'service_account')
  AND t.expires_at > $1
  AND t.expires_at <= $1 + make_interval(days => $2)
  AND t.created_at <= t.expires_at - make_interval(days => $2)
  AND (t.legacy_api_key_id IS NULL
       OR EXISTS (SELECT 1 FROM api_keys k WHERE k.id = t.legacy_api_key_id AND k.is_active))
ORDER BY t.expires_at, t.id
LIMIT $3
"#;

/// The tokens whose notice is due now, soonest expiry first.
pub async fn due_notices<C: ConnectionTrait>(
    db: &C,
    now: DateTime<Utc>,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    let values = vec![now.into(), (NOTICE_DAYS as i32).into(), BATCH.into()];
    ApiTokens::find()
        .from_raw_sql(stmt(DUE_NOTICES_SQL, values))
        .all(db)
        .await
        .map_err(db_err("due expiry notices"))
}

const CLAIM_NOTICE_SQL: &str = "UPDATE api_tokens SET expiry_notified_at = now() \
     WHERE id = $1 AND expiry_notified_at IS NULL AND revoked_at IS NULL";
const RELEASE_NOTICE_SQL: &str = "UPDATE api_tokens SET expiry_notified_at = NULL WHERE id = $1";

/// Claim the notice for `token_id`: `true` when this caller is the one to
/// send it. Claimed before sending, so two drivers never both mail.
pub async fn claim_notice<C: ConnectionTrait>(db: &C, token_id: Uuid) -> Result<bool, OxyError> {
    let done = db
        .execute_raw(stmt(CLAIM_NOTICE_SQL, vec![token_id.into()]))
        .await
        .map_err(db_err("claim expiry notice"))?;
    Ok(done.rows_affected() == 1)
}

/// Give a claimed notice back after the mail failed, so the next pass retries.
pub async fn release_notice<C: ConnectionTrait>(db: &C, token_id: Uuid) -> Result<(), OxyError> {
    db.execute_raw(stmt(RELEASE_NOTICE_SQL, vec![token_id.into()]))
        .await
        .map_err(db_err("release expiry notice"))?;
    Ok(())
}

/// [`unused`] in SQL, for new-format kinds only and never a legacy mirror.
/// A token already expired is left as it is: its own expiry ended it, and
/// overwriting that would lose when.
const UNUSED_WHERE: &str = r#"
    revoked_at IS NULL
    AND kind IN ('personal', 'service_account', 'ci')
    AND legacy_api_key_id IS NULL
    AND (expires_at IS NULL OR expires_at > $1)
    AND GREATEST(created_at, last_used_at, renewed_at) < $1 - make_interval(days => $2)
"#;

/// The tokens the unused sweep would expire now, oldest first.
pub async fn unused_candidates<C: ConnectionTrait>(
    db: &C,
    now: DateTime<Utc>,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    let sql =
        format!("SELECT * FROM api_tokens WHERE {UNUSED_WHERE} ORDER BY created_at, id LIMIT $3");
    let values = vec![now.into(), (UNUSED_DAYS as i32).into(), BATCH.into()];
    ApiTokens::find()
        .from_raw_sql(stmt(&sql, values))
        .all(db)
        .await
        .map_err(db_err("unused tokens"))
}

/// Expire one unused token: `expires_at = now`, `expired_reason = unused`.
/// Re-checks the rule in the same statement, so a use or an extend since the
/// candidate was read wins. `None` when nothing changed.
pub async fn expire_unused<C: ConnectionTrait>(
    db: &C,
    token_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<api_tokens::Model>, OxyError> {
    let sql = format!(
        "UPDATE api_tokens SET expires_at = $1, expired_reason = '{UNUSED_REASON}' \
         WHERE id = $3 AND {UNUSED_WHERE} RETURNING *"
    );
    let values = vec![now.into(), (UNUSED_DAYS as i32).into(), token_id.into()];
    ApiTokens::find()
        .from_raw_sql(stmt(&sql, values))
        .one(db)
        .await
        .map_err(db_err("expire unused token"))
}

const RESET_ON_EXTEND_SQL: &str = "UPDATE api_tokens SET expiry_notified_at = NULL, \
     expired_reason = NULL, renewed_at = now() WHERE id = $1";
const MARK_RENEWED_SQL: &str = "UPDATE api_tokens SET renewed_at = now() WHERE id = $1";

/// The token was extended: a new expiry gets a new notice, no hygiene expiry
/// stays on record, and it is a year before it counts as unused again.
pub async fn reset_on_extend<C: ConnectionTrait>(db: &C, token_id: Uuid) -> Result<(), OxyError> {
    db.execute_raw(stmt(RESET_ON_EXTEND_SQL, vec![token_id.into()]))
        .await
        .map_err(db_err("reset token hygiene"))?;
    Ok(())
}

/// The token was regenerated: its expiry is unchanged, so the notice stands,
/// but it is a year before it counts as unused again.
pub async fn mark_renewed<C: ConnectionTrait>(db: &C, token_id: Uuid) -> Result<(), OxyError> {
    db.execute_raw(stmt(MARK_RENEWED_SQL, vec![token_id.into()]))
        .await
        .map_err(db_err("mark token renewed"))?;
    Ok(())
}

#[cfg(test)]
#[path = "hygiene_tests.rs"]
mod tests;
