//! A service account's tokens: mint, extend, regenerate, revoke, activity —
//! the checks, the write and its lifecycle audit row in one transaction.
//!
//! A service-account token is **always grant-bound**, inside the account's own
//! org, at no more than the account's role (the body rules are
//! `oxy_auth::token::account_access`; this adds the one check that needs the
//! database — a named workspace belongs to the org). Its events are written to
//! that one org's chain.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use entity::prelude::Workspaces;
use entity::{api_tokens, service_accounts, workspaces};
use oxy_app_core::audit::RequestActor;
use oxy_auth::ExtendTo;
use oxy_auth::token::StoredKind;
use oxy_auth::token::account_access::CreateAccountTokenBody;
use oxy_auth::token::personal::{self, GrantSpec};
use oxy_auth::token::service_account::{self, NewAccountToken};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::server::api::api_keys::activity::{ActivityResponse, last_used_at, token_activity};
use crate::server::api::user_tokens::audit::{self as token_audit, Event};
use crate::server::api::user_tokens::dto::TokenDto;
use crate::server::api::user_tokens::error::TokenError;
use crate::server::api::user_tokens::{policy_cap, view};

/// A token and the secret that is shown exactly once.
pub(super) struct Minted {
    pub token: TokenDto,
    pub secret: String,
}

/// One token of this account, or 404 — any other token reads the same as none.
async fn find<C: ConnectionTrait>(
    db: &C,
    account: &service_accounts::Model,
    token_id: Uuid,
) -> Result<api_tokens::Model, TokenError> {
    service_account::find_token(db, account.user_id, token_id)
        .await?
        .ok_or(TokenError::NotFound)
}

fn invalidate(token_id: Uuid) {
    oxy_auth::token::cache::invalidate_token(token_id);
}

/// The wire form of the account's tokens: owned by the account, by name.
async fn dtos<C: ConnectionTrait>(
    db: &C,
    account: &service_accounts::Model,
    rows: &[api_tokens::Model],
) -> Result<Vec<TokenDto>, TokenError> {
    let owners = HashMap::from([(account.user_id, account.name.clone())]);
    view::tokens(db, rows, &owners).await
}

async fn dto<C: ConnectionTrait>(
    db: &C,
    account: &service_accounts::Model,
    row: &api_tokens::Model,
) -> Result<TokenDto, TokenError> {
    let mut out = dtos(db, account, std::slice::from_ref(row)).await?;
    out.pop()
        .ok_or_else(|| TokenError::Internal("a token row mapped to nothing".into()))
}

/// What every event of an account's token says about the account.
fn account_detail(account: &service_accounts::Model) -> Value {
    json!({
        "service_account_id": account.user_id,
        "service_account": account.name,
    })
}

fn with(mut detail: Value, extra: Value) -> Value {
    if let (Value::Object(detail), Value::Object(extra)) = (&mut detail, extra) {
        detail.extend(extra);
    }
    detail
}

/// One lifecycle event of an account's token, in the account's org.
async fn record<C: ConnectionTrait>(
    txn: &C,
    actor: &RequestActor,
    account: &service_accounts::Model,
    action: &'static str,
    token: &api_tokens::Model,
    extra: Value,
    change: Option<(Value, Value)>,
) -> Result<(), TokenError> {
    Event {
        action,
        token,
        orgs: vec![account.org_id],
        detail: with(account_detail(account), extra),
        change,
    }
    .record(txn, actor)
    .await
}

/// The `token.revoked` row of a token that went with its account.
pub(super) async fn record_revoked<C: ConnectionTrait>(
    txn: &C,
    actor: &RequestActor,
    account: &service_accounts::Model,
    token: &api_tokens::Model,
) -> Result<(), TokenError> {
    let reason = json!({ "reason": service_account::REVOKED_WITH_ACCOUNT });
    record(
        txn,
        actor,
        account,
        token_audit::REVOKED,
        token,
        reason,
        None,
    )
    .await
}

/// Every workspace a grant names is one of this org's. Anything else is 404,
/// so naming an id is not a way to learn whether a workspace exists.
pub(super) async fn check_workspaces(
    db: &DatabaseConnection,
    org_id: Uuid,
    grants: &[GrantSpec],
) -> Result<(), TokenError> {
    let wanted: HashSet<Uuid> = grants.iter().filter_map(|g| g.workspace_id).collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let found = Workspaces::find()
        .filter(workspaces::Column::Id.is_in(wanted.iter().copied()))
        .filter(workspaces::Column::OrgId.eq(org_id))
        .all(db)
        .await?;
    if found.len() == wanted.len() {
        Ok(())
    } else {
        Err(TokenError::NotFound)
    }
}

pub(super) async fn list(
    db: &DatabaseConnection,
    account: &service_accounts::Model,
) -> Result<Vec<TokenDto>, TokenError> {
    let rows = service_account::tokens(db, account.user_id).await?;
    dtos(db, account, &rows).await
}

pub(super) async fn create(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    body: CreateAccountTokenBody,
) -> Result<Minted, TokenError> {
    let role = service_account::role_of(account)
        .ok_or_else(|| TokenError::Internal("service account has an unknown org_role".into()))?;
    let name = body.name()?;
    let grants = body.grants(account.org_id, role)?;
    let expires_at = body.expires_at(Utc::now())?;
    check_workspaces(db, account.org_id, &grants).await?;
    let shape = policy_cap::new_token(StoredKind::ServiceAccount, false);
    policy_cap::check(db, &shape, &[account.org_id], expires_at).await?;

    let txn = db.begin().await?;
    let new = NewAccountToken::from_ui(name, grants, expires_at, actor.id);
    let minted = service_account::mint(&txn, account, new).await?;
    let stored = personal::grants_for(&txn, &[minted.row.id]).await?;
    let mut detail = token_audit::access_summary(&minted.row, &stored);
    detail["expires_at"] = token_audit::rfc3339(minted.row.expires_at);
    record(
        &txn,
        actor,
        account,
        token_audit::CREATED,
        &minted.row,
        detail,
        None,
    )
    .await?;
    txn.commit().await?;
    Ok(Minted {
        token: dto(db, account, &minted.row).await?,
        secret: minted.secret,
    })
}

/// Push out the expiry. Revives an expired token; never a revoked one.
pub(super) async fn extend(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    token_id: Uuid,
    target: ExtendTo,
) -> Result<TokenDto, TokenError> {
    let row = find(db, account, token_id).await?;
    if row.revoked_at.is_some() {
        return Err(TokenError::Revoked);
    }
    let previous = row.expires_at;
    let at = target
        .resolve(previous.map(Into::into), Utc::now())
        .map_err(TokenError::Invalid)?;
    policy_cap::check_row(db, &row, at).await?;
    let txn = db.begin().await?;
    let updated = personal::set_expiry(&txn, row, at).await?;
    let (old, new) = (
        token_audit::rfc3339(previous),
        token_audit::rfc3339(updated.expires_at),
    );
    let detail = json!({ "old_expires_at": old, "new_expires_at": new });
    let change = Some((json!({ "expires_at": old }), json!({ "expires_at": new })));
    record(
        &txn,
        actor,
        account,
        token_audit::EXTENDED,
        &updated,
        detail,
        change,
    )
    .await?;
    txn.commit().await?;
    invalidate(updated.id);
    dto(db, account, &updated).await
}

/// A new `oxy_sat_` secret for the same token: same id, grants and expiry.
pub(super) async fn regenerate(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    token_id: Uuid,
) -> Result<Minted, TokenError> {
    let row = find(db, account, token_id).await?;
    if row.revoked_at.is_some() {
        return Err(TokenError::Revoked);
    }
    policy_cap::check_row(db, &row, row.expires_at.map(Into::into)).await?;
    let txn = db.begin().await?;
    let minted = personal::regenerate(&txn, row).await?;
    record(
        &txn,
        actor,
        account,
        token_audit::REGENERATED,
        &minted.row,
        json!({}),
        None,
    )
    .await?;
    txn.commit().await?;
    invalidate(minted.row.id);
    Ok(Minted {
        token: dto(db, account, &minted.row).await?,
        secret: minted.secret,
    })
}

/// Revoke. Idempotent: an already-revoked token records nothing.
pub(super) async fn revoke(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    token_id: Uuid,
) -> Result<(), TokenError> {
    let row = find(db, account, token_id).await?;
    let txn = db.begin().await?;
    if let Some(token) = personal::revoke(&txn, row, actor.id, "org_admin").await? {
        let reason = json!({ "reason": "org_admin" });
        record(
            &txn,
            actor,
            account,
            token_audit::REVOKED,
            &token,
            reason,
            None,
        )
        .await?;
    }
    txn.commit().await?;
    invalidate(token_id);
    Ok(())
}

/// The token's whole Activity: everything a service account does is in its org.
pub(super) async fn activity(
    db: &DatabaseConnection,
    account: &service_accounts::Model,
    token_id: Uuid,
    limit: u64,
) -> Result<ActivityResponse, TokenError> {
    let row = find(db, account, token_id).await?;
    let fallback = row.last_used_at.map(|at| last_used_at(at.into()));
    Ok(token_activity(db, row.id, fallback, limit).await?)
}
