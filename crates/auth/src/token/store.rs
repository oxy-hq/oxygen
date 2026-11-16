//! Database side of token authentication: look up, admit, touch.
//!
//! `api_tokens` is the only source of truth. A row found by hash is admitted
//! if, and only if, [`admit`] agrees.
//!
//! **There is no fallback any more.** Legacy API keys were removed, and with
//! them the plaintext `api_keys` lookup that let a key a pod one release back
//! had minted authenticate without a mirror row. `api_keys` is kept as a table
//! — dropping it would break a revert — but nothing here reads it to decide a
//! request, and every surviving row was revoked by the removal migration.

use chrono::{DateTime, Utc};
use entity::prelude::{ApiTokenGrants, ApiTokens, ServiceAccounts};
use entity::{api_token_grants, api_tokens};
use oxy_shared::errors::OxyError;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseBackend, DatabaseConnection, EntityTrait, QueryFilter,
    Statement,
};

use super::credential::{
    AccountLink, CredentialContext, Links, StoredKind, admit, place_sandbox_apps,
};
use super::format::{TokenFormat, hash_token};
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
/// the caller is treating it as.
///
/// One lookup, one answer: no row, or an unreadable `api_tokens`, is a refusal.
/// Every format that reaches here is a checksummed one, so there is nothing
/// older to be kind to.
pub async fn resolve(
    db: &DatabaseConnection,
    token: &str,
    presented: TokenFormat,
) -> Result<Resolved, OxyError> {
    let hash = hash_token(token);
    match find_by_hash(db, &hash).await? {
        Some(row) => resolve_row(db, row, presented).await,
        None => Err(invalid("no token row for the presented token")),
    }
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
    let links = Links::account(account_link(db, &row).await?);
    let grants = grants_to_admit(db, &row).await?;
    let mut credential = admit(&row, &grants, presented, links, Utc::now()).map_err(invalid)?;
    block_by_policy(db, &row, &grants, &mut credential).await?;
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

const TOUCH_TOKEN_SQL: &str = "UPDATE api_tokens SET last_used_at = now() WHERE id = $1 \
     AND (last_used_at IS NULL OR last_used_at < now() - interval '5 minutes')";

/// Record a use: one conditional UPDATE, so a busy token costs one write per
/// five minutes. Best-effort — a failed write is logged, never a failed
/// request. The matching `api_keys` write went with legacy keys; no admitted
/// credential mirrors that table any more.
pub async fn touch_last_used(db: &DatabaseConnection, credential: &CredentialContext) {
    let stmt = Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        TOUCH_TOKEN_SQL,
        [credential.token_id.into()],
    );
    if let Err(e) = db.execute_raw(stmt).await {
        tracing::warn!(token_id = %credential.token_id, error = %e, "could not record token use");
    }
}
