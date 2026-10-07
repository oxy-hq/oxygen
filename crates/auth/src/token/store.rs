//! Database side of token authentication: look up, fall back, mirror, touch.
//!
//! Release N of design §7. `api_tokens` is the source of truth for a token's
//! hash, kind and reach, and `api_keys` stays authoritative for what a pod one
//! release back can do to a legacy key:
//!
//! - a row found by hash is admitted only if [`admit`] agrees **and**, when it
//!   mirrors an `api_keys` row, that row is still active (an older pod revokes
//!   by writing `api_keys` alone);
//! - a legacy-format key with no row falls back to today's plaintext lookup in
//!   `api_keys` (an older pod may have minted it) and is mirrored on the spot.
//!   A failure to mirror never fails the request: the key is valid by today's
//!   rule, and today's rule is the one this release must not break.
//!
//! A new-prefix token has no fallback. Nothing but this release mints one, and
//! it always writes the row.

use chrono::{DateTime, Utc};
use entity::prelude::{ApiKeys, ApiTokenGrants, ApiTokens, ServiceAccounts};
use entity::{api_keys, api_token_grants, api_tokens};
use oxy_shared::errors::OxyError;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait,
    QueryFilter, Statement,
};
use uuid::Uuid;

use super::credential::{
    AccountLink, CredentialContext, LegacyLink, Links, StoredKind, admit, place_sandbox_apps,
    source,
};
use super::format::{TokenFormat, display_prefix, hash_token};
use super::{policy_store, sandbox};
use crate::api_key_infra::{identity_for_key_owner, identity_for_service_account};
use crate::types::Identity;

/// A credential resolved from the database.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub identity: Identity,
    pub credential: CredentialContext,
    pub expires_at: Option<DateTime<Utc>>,
}

fn db_err(what: &str) -> impl FnOnce(sea_orm::DbErr) -> OxyError + '_ {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

pub(super) fn invalid(reason: impl std::fmt::Display) -> OxyError {
    tracing::debug!(%reason, "API token refused");
    OxyError::AuthenticationError("Invalid API key".to_string())
}

/// Resolve a presented token (already trimmed). `presented` says which format
/// the caller is treating it as; any non-prefixed `X-API-Key` value is a
/// [`TokenFormat::LegacyKey`].
///
/// A legacy key survives `api_tokens` being unreadable (a migration that did
/// not run, say): the lookup error is logged and today's `api_keys` path
/// decides, as it did before this table existed. Every revoke writes
/// `api_keys` too, so that path never honours a key the owner revoked.
pub async fn resolve(
    db: &DatabaseConnection,
    token: &str,
    presented: TokenFormat,
) -> Result<Resolved, OxyError> {
    let hash = hash_token(token);
    match find_by_hash(db, &hash).await {
        Ok(Some(row)) => return resolve_row(db, row, presented).await,
        Ok(None) if presented.is_new() => {
            return Err(invalid("no token row for a new-format token"));
        }
        Err(e) if presented.is_new() => return Err(e),
        Ok(None) => {}
        Err(e) => {
            tracing::error!(error = %e, "api_tokens unreadable; validating the legacy key from api_keys");
        }
    }
    resolve_legacy_fallback(db, token, &hash).await
}

async fn find_by_hash(
    db: &DatabaseConnection,
    hash: &[u8],
) -> Result<Option<api_tokens::Model>, OxyError> {
    ApiTokens::find()
        .filter(api_tokens::Column::TokenHash.eq(hash.to_vec()))
        .one(db)
        .await
        .map_err(db_err("api token lookup"))
}

pub(super) async fn resolve_row(
    db: &DatabaseConnection,
    row: api_tokens::Model,
    presented: TokenFormat,
) -> Result<Resolved, OxyError> {
    let links = Links {
        legacy: legacy_link(db, row.legacy_api_key_id).await?,
        account: account_link(db, &row).await?,
    };
    let grants = grants_to_admit(db, &row).await?;
    let mut credential = admit(&row, &grants, presented, links, Utc::now()).map_err(invalid)?;
    if !credential.is_legacy() {
        block_by_policy(db, &row, &grants, &mut credential).await?;
    }
    if credential.is_sandbox_agent() {
        // Its workspace grants, from where each app it names lives now. One
        // read, for this kind only; a failed one fails the token closed.
        let homes = sandbox::app_homes(db, &credential.app_sandbox).await?;
        place_sandbox_apps(&mut credential, &homes);
    }
    let identity = if credential.is_service_account() {
        identity_for_service_account(db, row.principal_user_id).await?
    } else {
        identity_for_key_owner(db, row.principal_user_id).await?
    };
    Ok(Resolved {
        identity,
        credential,
        expires_at: row.expires_at.map(DateTime::<Utc>::from),
    })
}

/// Add the orgs whose token policy this credential violates to the orgs it
/// reaches nothing in (design §5): inert there, never refused outright. Never
/// called for a legacy credential. A failed read fails the token closed, as a
/// failed grants read does.
async fn block_by_policy(
    db: &DatabaseConnection,
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
    credential: &mut CredentialContext,
) -> Result<(), OxyError> {
    for (org, _) in policy_store::blocks(db, row, grants).await? {
        if !credential.blocked_orgs.contains(&org) {
            credential.blocked_orgs.push(org);
        }
    }
    Ok(())
}

/// The grants admission must see. Only a row that can be narrowed is looked
/// up: a legacy key (or a legacy-endpoint token) never carries a grant, so its
/// validation never depends on `api_token_grants` being readable — nothing
/// added for narrowing may stand between an existing key and its request. A
/// failed read for any other row is an error, so the token fails closed.
async fn grants_to_admit(
    db: &DatabaseConnection,
    row: &api_tokens::Model,
) -> Result<Vec<api_token_grants::Model>, OxyError> {
    let legacy = row.kind == StoredKind::LegacyKey.as_str() || row.legacy_api_key_id.is_some();
    if legacy {
        return Ok(Vec::new());
    }
    ApiTokenGrants::find()
        .filter(api_token_grants::Column::TokenId.eq(row.id))
        .all(db)
        .await
        .map_err(db_err("api token grants lookup"))
}

/// The `service_accounts` row behind a token that acts as an account — an
/// `oxy_sat_` token, or the `oxy_ci_` token a trust policy minted. Looked up
/// for those kinds only, so nothing here stands between a person's token — or
/// a legacy key — and its request.
async fn account_link(
    db: &DatabaseConnection,
    row: &api_tokens::Model,
) -> Result<AccountLink, OxyError> {
    if !StoredKind::parse(&row.kind).is_some_and(StoredKind::acts_as_account) {
        return Ok(AccountLink::NotAccount);
    }
    let account = ServiceAccounts::find_by_id(row.principal_user_id)
        .one(db)
        .await
        .map_err(db_err("service account lookup"))?;
    Ok(AccountLink::of(account.as_ref()))
}

/// Re-read the account behind a **cached** service-account credential.
///
/// The ≤30 s cache must not keep a disabled account's tokens alive, nor an
/// admin standing the org took back, on any pod — so the account row is read
/// on every request, and the cached credential carries what the row says now.
/// One primary-key read, after the one every cached credential pays for its
/// principal's status (`require_active_owner`).
pub async fn refresh_account(
    db: &DatabaseConnection,
    mut credential: CredentialContext,
) -> Result<CredentialContext, OxyError> {
    let account = ServiceAccounts::find_by_id(credential.principal_user_id)
        .one(db)
        .await
        .map_err(db_err("service account lookup"))?;
    match AccountLink::of(account.as_ref()) {
        AccountLink::Active(standing) => {
            credential.service_account = Some(standing);
            Ok(credential)
        }
        link => Err(invalid(format!("service account is not active: {link:?}"))),
    }
}

async fn legacy_link(db: &DatabaseConnection, id: Option<Uuid>) -> Result<LegacyLink, OxyError> {
    let Some(id) = id else {
        return Ok(LegacyLink::NotLinked);
    };
    let key = ApiKeys::find_by_id(id)
        .one(db)
        .await
        .map_err(db_err("api key link lookup"))?;
    Ok(match key {
        None => LegacyLink::Missing,
        Some(k) if k.is_active => LegacyLink::Active,
        Some(_) => LegacyLink::Inactive,
    })
}

/// Today's lookup, unchanged: the plaintext in `api_keys.key_hash`, active and
/// not expired.
pub async fn find_active_legacy_key(
    db: &DatabaseConnection,
    key: &str,
) -> Result<Option<api_keys::Model>, OxyError> {
    let now = Utc::now().fixed_offset();
    ApiKeys::find()
        .filter(api_keys::Column::KeyHash.eq(key))
        .filter(api_keys::Column::IsActive.eq(true))
        .filter(
            Condition::any()
                .add(api_keys::Column::ExpiresAt.is_null())
                .add(api_keys::Column::ExpiresAt.gt(now)),
        )
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("Database error: {e}")))
}

async fn resolve_legacy_fallback(
    db: &DatabaseConnection,
    key: &str,
    hash: &[u8],
) -> Result<Resolved, OxyError> {
    let Some(api_key) = find_active_legacy_key(db, key).await? else {
        return Err(invalid("no api_tokens row and no active api_keys row"));
    };
    if let Err(e) = ensure_legacy_row(db, api_key.id, source::LEGACY_LAZY).await {
        tracing::warn!(api_key_id = %api_key.id, error = %e, "could not mirror a legacy key into api_tokens");
    }
    // Re-read so the mirrored row goes through the same admission as any other.
    match find_by_hash(db, hash).await {
        Ok(Some(row)) => return resolve_row(db, row, TokenFormat::LegacyKey).await,
        Ok(None) => {}
        Err(e) => tracing::warn!(error = %e, "could not re-read a mirrored legacy key"),
    }
    // Mirroring failed. The key is valid by today's rule, so honour it with
    // today's reach rather than break it over a side table.
    let identity = identity_for_key_owner(db, api_key.user_id).await?;
    Ok(Resolved {
        identity,
        credential: legacy_credential(&api_key),
        expires_at: api_key.expires_at.map(DateTime::<Utc>::from),
    })
}

/// The credential a legacy key carries: everything it reaches today.
fn legacy_credential(api_key: &api_keys::Model) -> CredentialContext {
    CredentialContext {
        token_id: api_key.id,
        kind: StoredKind::LegacyKey,
        principal_user_id: api_key.user_id,
        all_access: true,
        platform: true,
        partner: true,
        name: api_key.name.clone(),
        display_prefix: display_prefix(&api_key.key_hash),
        legacy_api_key_id: Some(api_key.id),
        grants: Vec::new(),
        blocked_orgs: Vec::new(),
        service_account: None,
        app_publish: Vec::new(),
        app_sandbox: Vec::new(),
        expires_at: api_key.expires_at.map(DateTime::<Utc>::from),
    }
}

/// The `api_keys` → `api_tokens` mapping, stated once for every runtime mirror:
/// the columns a mirrored key fills, in insert order.
///
/// The migration's backfill (`m20261001_000001_api_tokens::BACKFILL_SQL`) keeps
/// its own copy, because a migration is a frozen snapshot. A test in this
/// module fails if the two differ.
const LEGACY_MIRROR_COLUMNS: &str = r#"
    id, kind, principal_user_id, name, display_prefix, last_four, token_hash,
    all_access, platform, partner, expires_at, last_used_at, created_at,
    created_by, revoked_at, source, legacy_api_key_id
"#;

/// The expression over `api_keys k` that fills each column above, in the same
/// order. `$2` is the row's `source`.
const LEGACY_MIRROR_SELECT: &str = r#"
    k.id, 'legacy_key', k.user_id, k.name,
    left(k.key_hash, 4),
    CASE WHEN length(k.key_hash) > 8 THEN right(k.key_hash, 4) ELSE '' END,
    sha256(convert_to(k.key_hash, 'UTF8')),
    true, true, true,
    k.expires_at, k.last_used_at, k.created_at,
    k.user_id,
    CASE WHEN k.is_active THEN NULL ELSE k.updated_at END,
    $2::text, k.id
"#;

/// One `api_keys` row, by id.
const MIRROR_ONE_KEY: &str = "k.id = $1::uuid";
/// Every `api_keys` row of one user — only a key with no mirror yet inserts,
/// i.e. one a pod one release back minted and nobody has used since.
const MIRROR_USERS_KEYS: &str = "k.user_id = $1::uuid";

/// The mirror statement for the `api_keys` rows `filter` selects: `$1` is the
/// filter's one parameter and `$2` the `source` to stamp. A row that already
/// has its mirror is left alone.
fn mirror_legacy_keys_sql(filter: &str) -> String {
    format!(
        "INSERT INTO api_tokens ({LEGACY_MIRROR_COLUMNS})\n\
         SELECT {LEGACY_MIRROR_SELECT}\n\
         FROM api_keys k\n\
         WHERE {filter}\n\
         ON CONFLICT DO NOTHING\n"
    )
}

/// Make sure an `api_keys` row has its `api_tokens` mirror. A no-op when it
/// already has one (including a token the legacy endpoint dual-wrote).
pub async fn ensure_legacy_row<C: ConnectionTrait>(
    db: &C,
    api_key_id: Uuid,
    source: &str,
) -> Result<(), OxyError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        mirror_legacy_keys_sql(MIRROR_ONE_KEY),
        [api_key_id.into(), source.into()],
    ))
    .await
    .map_err(db_err("mirror api key into api_tokens"))?;
    Ok(())
}

/// Mirror every legacy API key of `user_id` that has no `api_tokens` row yet.
/// Called when the user lists their legacy keys, so activity, the org
/// inventory and the expiry notice see a key before its first use.
pub async fn ensure_user_legacy_rows<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<(), OxyError> {
    db.execute_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        mirror_legacy_keys_sql(MIRROR_USERS_KEYS),
        [user_id.into(), source::LEGACY_LAZY.into()],
    ))
    .await
    .map_err(db_err("mirror a user's api keys into api_tokens"))?;
    Ok(())
}

const TOUCH_TOKEN_SQL: &str = "UPDATE api_tokens SET last_used_at = now() WHERE id = $1 \
     AND (last_used_at IS NULL OR last_used_at < now() - interval '5 minutes')";
const TOUCH_KEY_SQL: &str = "UPDATE api_keys SET last_used_at = now() WHERE id = $1 \
     AND (last_used_at IS NULL OR last_used_at < now() - interval '5 minutes')";

/// Record a use: one conditional UPDATE per table, so a busy token costs one
/// write per five minutes. The `api_keys` mirror is what makes the API Keys
/// page's Last-used column true. Best-effort: a failed write is logged, never
/// a failed request.
pub async fn touch_last_used(db: &DatabaseConnection, credential: &CredentialContext) {
    let mut writes = vec![(TOUCH_TOKEN_SQL, credential.token_id)];
    if let Some(key_id) = credential.legacy_api_key_id {
        writes.push((TOUCH_KEY_SQL, key_id));
    }
    for (sql, id) in writes {
        let stmt = Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, [id.into()]);
        if let Err(e) = db.execute_raw(stmt).await {
            tracing::warn!(token_id = %credential.token_id, error = %e, "could not record token use");
        }
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
