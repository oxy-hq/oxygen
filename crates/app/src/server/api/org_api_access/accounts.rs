//! What each `/api/orgs/{org_id}/service-accounts` route does: the checks, the
//! write and its audit row in one transaction, then the credential cache.
//!
//! The cache matters twice here. Disabling or deleting an account must stop
//! its tokens at once: authentication re-reads the account row on every
//! request, on every pod, so that needs nothing from this module — and this
//! pod's cached credentials are dropped anyway, so the next request does not
//! even reach that read with a stale entry.

use std::collections::{HashMap, HashSet};

use entity::prelude::Users;
use entity::{service_accounts, users};
use oxy_app_core::audit::RequestActor;
use oxy_auth::token::account_access::{CreateAccountBody, PatchAccountBody};
use oxy_auth::token::service_account::{self, CreateError};
use oxy_auth::token::trust_policy;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
};
use serde_json::json;
use uuid::Uuid;

use super::audit;
use super::dto::{CreatorDto, ServiceAccountDto, account_dto};
use crate::server::api::user_tokens::error::TokenError;

impl From<CreateError> for TokenError {
    fn from(e: CreateError) -> Self {
        match e {
            CreateError::NameTaken => Self::NameTaken,
            CreateError::Db(e) => e.into(),
        }
    }
}

/// One account of this org, or 404 — another org's reads the same as none.
pub(super) async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    account_id: Uuid,
) -> Result<service_accounts::Model, TokenError> {
    service_account::find(db, org_id, account_id)
        .await?
        .ok_or(TokenError::NotFound)
}

/// The account of this org as it is now, under its row lock: what a write
/// decides from. Taken first in the write's transaction; gone, or another
/// org's, is 404 — the answer [`find`] gives. Never a row read before the
/// transaction, which another request may have changed since.
async fn locked<C: ConnectionTrait>(
    txn: &C,
    org_id: Uuid,
    account_id: Uuid,
) -> Result<service_accounts::Model, TokenError> {
    service_account::lock(txn, org_id, account_id)
        .await?
        .ok_or(TokenError::NotFound)
}

/// Who created each account, for display.
async fn creators<C: ConnectionTrait>(
    db: &C,
    rows: &[service_accounts::Model],
) -> Result<HashMap<Uuid, CreatorDto>, TokenError> {
    let ids: HashSet<Uuid> = rows.iter().filter_map(|r| r.created_by).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let users = Users::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await?;
    Ok(users
        .into_iter()
        .map(|u| {
            let label = u.label().to_string();
            (u.id, CreatorDto { id: u.id, label })
        })
        .collect())
}

/// The wire form of `rows`, in order: two reads, whatever their number.
pub(super) async fn dtos<C: ConnectionTrait>(
    db: &C,
    rows: &[service_accounts::Model],
) -> Result<Vec<ServiceAccountDto>, TokenError> {
    let creators = creators(db, rows).await?;
    let ids: Vec<Uuid> = rows.iter().map(|r| r.user_id).collect();
    let counts = service_account::token_counts(db, &ids).await?;
    let policies = trust_policy::counts(db, &ids).await?;
    Ok(rows
        .iter()
        .map(|row| {
            let creator = row.created_by.and_then(|id| creators.get(&id).cloned());
            let count = counts.get(&row.user_id).copied().unwrap_or(0);
            let policy_count = policies.get(&row.user_id).copied().unwrap_or(0);
            account_dto(row, creator, count, policy_count)
        })
        .collect())
}

async fn dto<C: ConnectionTrait>(
    db: &C,
    row: &service_accounts::Model,
) -> Result<ServiceAccountDto, TokenError> {
    let mut out = dtos(db, std::slice::from_ref(row)).await?;
    out.pop()
        .ok_or_else(|| TokenError::Internal("a service account mapped to nothing".into()))
}

pub(super) async fn list(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<ServiceAccountDto>, TokenError> {
    let rows = service_account::list(db, org_id).await?;
    dtos(db, &rows).await
}

pub(super) async fn get(
    db: &DatabaseConnection,
    org_id: Uuid,
    account_id: Uuid,
) -> Result<ServiceAccountDto, TokenError> {
    let row = find(db, org_id, account_id).await?;
    dto(db, &row).await
}

pub(super) async fn create(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org_id: Uuid,
    body: CreateAccountBody,
) -> Result<ServiceAccountDto, TokenError> {
    let want = body.want()?;
    let txn = db.begin().await?;
    let row = service_account::create(&txn, org_id, want, actor.id).await?;
    audit::record(
        &txn,
        actor,
        audit::CREATED,
        &row,
        audit::summary(&row),
        None,
    )
    .await?;
    txn.commit().await?;
    dto(db, &row).await
}

/// Drop this pod's cached credentials of the account's tokens.
async fn invalidate_tokens(db: &DatabaseConnection, account_id: Uuid) -> Result<(), TokenError> {
    for token in service_account::every_token(db, account_id).await? {
        oxy_auth::token::cache::invalidate_token(token.id);
    }
    Ok(())
}

pub(super) async fn patch(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org_id: Uuid,
    account_id: Uuid,
    body: PatchAccountBody,
) -> Result<ServiceAccountDto, TokenError> {
    let edit = body.edit()?;
    let txn = db.begin().await?;
    // The account as it is under its row lock, taken first: what the edit is
    // judged against. Read before the transaction it can be stale — off, on,
    // off in quick succession, and the second "off" would see the account
    // still disabled, change nothing, and answer 200 for one that is enabled.
    // The lock also waits out a mint in flight for one of its policies.
    let before = locked(&txn, org_id, account_id).await?;
    let after = service_account::update(&txn, before.clone(), &edit).await?;
    if after != before {
        let action = audit::edit_action(&before, &after);
        let change = Some((audit::summary(&before), audit::summary(&after)));
        audit::record(&txn, actor, action, &after, json!({}), change).await?;
    }
    txn.commit().await?;
    // The role and the disabled state both ride the cached credential.
    invalidate_tokens(db, account_id).await?;
    dto(db, &after).await
}

/// Delete the account. Its live tokens are revoked with it, each with its own
/// `token.revoked` row beside the account's `deleted` one.
pub(super) async fn delete(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org_id: Uuid,
    account_id: Uuid,
) -> Result<(), TokenError> {
    let txn = db.begin().await?;
    // The lock first, and the row it hands back. An account that is already
    // gone — a second delete crossing the first — is not found here, before
    // anything is revoked, deleted or audited.
    let row = locked(&txn, org_id, account_id).await?;
    let revoked = service_account::delete(&txn, row.clone(), actor.id).await?;
    for token in &revoked {
        super::account_tokens::record_revoked(&txn, actor, &row, token).await?;
    }
    let detail = json!({ "revoked_tokens": revoked.len() });
    let before = Some((audit::summary(&row), json!(null)));
    audit::record(&txn, actor, audit::DELETED, &row, detail, before).await?;
    txn.commit().await?;
    for token in &revoked {
        oxy_auth::token::cache::invalidate_token(token.id);
    }
    Ok(())
}
