//! Leak response: what `POST /api/auth/tokens/revoke-leaked` does with each
//! reported value (API-tokens design §8 Phase 5).
//!
//! The endpoint is public — a secret scanner reports what it found, and the
//! report is the only credential — so each value is judged **offline first**:
//!
//! 1. A value that is not a well-formed new-format token, checksum included,
//!    is `unknown` with no database read. A lookalike costs nothing and learns
//!    nothing.
//! 2. A legacy `oxy_<hex>` key is `ignored_legacy`, also with no read. Leak
//!    revocation never touches a legacy key (§3.5): only its owner can end it.
//! 3. Otherwise the row is looked up by hash and [`decide`] says what to do.
//!    A row that mirrors `api_keys` — a token the legacy endpoint minted — is
//!    a legacy key too, and is `ignored_legacy`.
//!
//! The answer names the token by its non-secret display prefix only.

use entity::api_tokens;
use entity::prelude::ApiTokens;
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter};

use super::credential::StoredKind;
use super::format::{TokenFormat, hash_token, parse_format, verify_checksum};

/// The most values one report may carry.
pub const MAX_REPORTS: usize = 100;
/// `api_tokens.revoke_reason`, and `metadata.reason` on `token.revoked`.
pub const REVOKE_REASON: &str = "leaked";

/// What a reported value is, judged without the database.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Presented {
    /// A well-formed `oxy_pat_` / `oxy_sat_` / `oxy_ci_` token: worth a lookup.
    NewFormat,
    /// An `oxy_<32 hex>` legacy key: never looked up, never revoked.
    Legacy,
    /// Anything else, including a new-format lookalike whose checksum fails.
    Unknown,
}

pub fn classify(token: &str) -> Presented {
    let token = token.trim();
    if verify_checksum(token) {
        Presented::NewFormat
    } else if parse_format(token) == Some(TokenFormat::LegacyKey) {
        Presented::Legacy
    } else {
        Presented::Unknown
    }
}

/// The per-value answer. The wire strings are the contract's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeakStatus {
    Revoked,
    AlreadyRevoked,
    Unknown,
    IgnoredLegacy,
}

impl LeakStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::AlreadyRevoked => "already_revoked",
            Self::Unknown => "unknown",
            Self::IgnoredLegacy => "ignored_legacy",
        }
    }
}

/// What to do with a looked-up row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Answer this, and write nothing.
    Answer(LeakStatus),
    /// Revoke it, then answer [`LeakStatus::Revoked`].
    Revoke,
}

/// What a new-format value's row calls for. An expired token is revoked too:
/// Extend could otherwise revive the exposed secret.
pub fn decide(row: Option<&api_tokens::Model>) -> Decision {
    let Some(row) = row else {
        return Decision::Answer(LeakStatus::Unknown);
    };
    let kind = StoredKind::parse(&row.kind);
    if kind == Some(StoredKind::LegacyKey) || row.legacy_api_key_id.is_some() {
        return Decision::Answer(LeakStatus::IgnoredLegacy);
    }
    if kind.is_none() {
        return Decision::Answer(LeakStatus::Unknown);
    }
    if row.revoked_at.is_some() {
        return Decision::Answer(LeakStatus::AlreadyRevoked);
    }
    Decision::Revoke
}

/// The row a presented token hashes to.
pub async fn find<C: ConnectionTrait>(
    db: &C,
    token: &str,
) -> Result<Option<api_tokens::Model>, OxyError> {
    ApiTokens::find()
        .filter(api_tokens::Column::TokenHash.eq(hash_token(token.trim())))
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("api token lookup: {e}")))
}

/// Revoke a reported token. Conditional, so of two reports racing only one
/// revokes; `None` when it was already revoked. Never a legacy row — the
/// statement refuses one whatever the caller decided — and `revoked_by` stays
/// empty: no person did it.
const REVOKE_SQL: &str = r#"
UPDATE api_tokens SET revoked_at = now(), revoke_reason = $2
WHERE id = $1
  AND revoked_at IS NULL
  AND legacy_api_key_id IS NULL
  AND kind IN ('personal', 'service_account', 'ci')
RETURNING *
"#;

pub async fn revoke<C: ConnectionTrait>(
    db: &C,
    token_id: uuid::Uuid,
) -> Result<Option<api_tokens::Model>, OxyError> {
    let stmt = sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        REVOKE_SQL,
        [token_id.into(), REVOKE_REASON.into()],
    );
    ApiTokens::find()
        .from_raw_sql(stmt)
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("revoke leaked token: {e}")))
}

#[cfg(test)]
#[path = "leak_tests.rs"]
mod tests;
