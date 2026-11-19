//! OIDC issuer trust policies: list (one account's, or the whole org's),
//! register, edit, delete (API-tokens design §3.4, *Any OIDC issuer*). The
//! handlers are in [`super::handlers`]; this is what they do, each write with
//! its audit row in the same transaction.
//!
//! The same contract as a GitHub policy's ([`super::trust_policies`]):
//!
//! - the issuer must be one this org registered; another org's reads as none;
//! - every grant is in the account's own org and never above its standing;
//! - never on a custom app's own service account (409 `app_account`);
//! - **any change to a policy ends the tokens it minted**, as does disabling
//!   or deleting it;
//! - each write that changes an existing policy decides from the row it
//!   locks, and its transaction asks the pool for nothing.
//!
//! Its audit rows are the GitHub policy's (`trust_policy.created` /
//! `updated` / `deleted`, target type `trust_policy`), with
//! `provider: "oidc_issuer"` and the conditions in the summary.

use std::collections::HashMap;

use entity::prelude::ServiceAccounts;
use entity::{api_tokens, oidc_issuer_trust_policies, oidc_issuers, service_accounts};
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::token::issuer_trust_policy::{self, NewIssuerPolicy, conditions_of};
use oxy_auth::token::issuer_trust_policy_access::{
    CreateIssuerPolicyBody, IssuerPolicyEdit, PatchIssuerPolicyBody,
};
use oxy_auth::token::{ci, oidc_issuer};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, DatabaseTransaction, EntityTrait,
    QueryFilter, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::dto::IssuerTrustPolicyDto;
use super::trust_policies::{check_targets, grant_dtos_of, grants_summary, invalidate, role_of};
use crate::user_tokens::error::TokenError;

const PROVIDER: &str = "oidc_issuer";
const TARGET_TYPE: &str = "trust_policy";
const CREATED: &str = "trust_policy.created";
const UPDATED: &str = "trust_policy.updated";
const DELETED: &str = "trust_policy.deleted";

type GrantRow = entity::oidc_trust_policy_grants::Model;

async fn issuers_of<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
) -> Result<HashMap<Uuid, oidc_issuers::Model>, TokenError> {
    Ok(oidc_issuer::list(db, org_id)
        .await?
        .into_iter()
        .map(|i| (i.id, i))
        .collect())
}

async fn accounts_of<C: ConnectionTrait>(
    db: &C,
    rows: &[oidc_issuer_trust_policies::Model],
) -> Result<HashMap<Uuid, service_accounts::Model>, TokenError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.service_account_id).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(ServiceAccounts::find()
        .filter(service_accounts::Column::UserId.is_in(ids))
        .all(db)
        .await?
        .into_iter()
        .map(|a| (a.user_id, a))
        .collect())
}

async fn dtos<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    rows: &[oidc_issuer_trust_policies::Model],
) -> Result<Vec<IssuerTrustPolicyDto>, TokenError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let grant_rows = issuer_trust_policy::grants_for(db, &ids).await?;
    let mut grants = grant_dtos_of(db, &grant_rows).await?;
    let issuers = issuers_of(db, org_id).await?;
    let accounts = accounts_of(db, rows).await?;
    let creator_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.created_by).collect();
    let creators = super::oidc_issuers::creators_by_id(db, &creator_ids).await?;
    rows.iter()
        .map(|row| {
            // The foreign keys keep a policy's issuer and account; one outside
            // the org is not a row this release writes.
            let missing =
                |what: &str| TokenError::Internal(format!("a trust policy's {what} is missing"));
            let issuer = issuers
                .get(&row.issuer_id)
                .ok_or_else(|| missing("issuer"))?;
            let account = accounts
                .get(&row.service_account_id)
                .ok_or_else(|| missing("account"))?;
            let creator = row.created_by.and_then(|id| creators.get(&id).cloned());
            let granted = grants.remove(&row.id).unwrap_or_default();
            Ok(IssuerTrustPolicyDto::of(
                row, account, issuer, granted, creator,
            ))
        })
        .collect()
}

async fn dto<C: ConnectionTrait>(
    db: &C,
    row: &oidc_issuer_trust_policies::Model,
) -> Result<IssuerTrustPolicyDto, TokenError> {
    let mut out = dtos(db, row.org_id, std::slice::from_ref(row)).await?;
    out.pop()
        .ok_or_else(|| TokenError::Internal("a trust policy mapped to nothing".into()))
}

/// One account's policies.
pub(super) async fn list(
    db: &DatabaseConnection,
    account: &service_accounts::Model,
) -> Result<Vec<IssuerTrustPolicyDto>, TokenError> {
    let rows = issuer_trust_policy::list(db, account.user_id).await?;
    dtos(db, account.org_id, &rows).await
}

/// Every account's policies in the org: what the settings screen lists under
/// each issuer.
pub(super) async fn list_org(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<IssuerTrustPolicyDto>, TokenError> {
    let rows = issuer_trust_policy::list_org(db, org_id).await?;
    dtos(db, org_id, &rows).await
}

fn summary(
    account: &service_accounts::Model,
    row: &oidc_issuer_trust_policies::Model,
    issuer: &str,
    grants: &[GrantRow],
) -> Value {
    json!({
        "service_account_id": account.user_id,
        "service_account": account.name,
        "provider": PROVIDER,
        "issuer_id": row.issuer_id,
        "issuer": issuer,
        "conditions": conditions_of(row),
        "disabled": row.disabled_at.is_some(),
        "grants": grants_summary(grants),
    })
}

fn entry(
    actor: &RequestActor,
    action: &'static str,
    row: &oidc_issuer_trust_policies::Model,
    issuer_name: &str,
    detail: Value,
) -> AuditEntry {
    let label = format!("{issuer_name} trust policy");
    AuditEntry::for_request(actor, action)
        .org(row.org_id)
        .target(TARGET_TYPE, row.id.to_string(), label)
        .metadata(detail)
}

/// The issuer **of this org**, or 404.
async fn issuer<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    issuer_id: Uuid,
) -> Result<oidc_issuers::Model, TokenError> {
    oidc_issuer::find(db, org_id, issuer_id)
        .await?
        .ok_or(TokenError::NotFound)
}

pub(super) async fn create(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    body: CreateIssuerPolicyBody,
) -> Result<IssuerTrustPolicyDto, TokenError> {
    let org_id = account.org_id;
    let want = body.want(org_id, role_of(account)?)?;
    // An app's account acts only through its approved actions, in-process. A
    // policy on it would let a workload mint tokens with the account's admin
    // standing and none of the approval's narrowing, as a GitHub policy would
    // (`trust_policies::write`). Only a create needs it: an app's account is
    // made new on its first approval, so it never holds a policy to edit.
    if oxy_auth::token::app_access::app_of_account(db, account.user_id)
        .await?
        .is_some()
    {
        return Err(TokenError::AppAccountFixed);
    }
    let issuer = issuer(db, org_id, want.issuer_id).await?;
    check_targets(db, org_id, &want.grants).await?;
    let new = NewIssuerPolicy {
        org_id,
        service_account_id: account.user_id,
        issuer_id: issuer.id,
        conditions: want.conditions,
        grants: want.grants,
        created_by: actor.id,
    };
    let txn = db.begin().await?;
    let row = writes::create(&txn, actor, account, &issuer, new).await?;
    txn.commit().await?;
    dto(db, &row).await
}

pub(super) async fn patch(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    policy_id: Uuid,
    body: PatchIssuerPolicyBody,
) -> Result<IssuerTrustPolicyDto, TokenError> {
    let org_id = account.org_id;
    let edit = body.edit(org_id, role_of(account)?)?;
    let row = issuer_trust_policy::find(db, account.user_id, policy_id)
        .await?
        .ok_or(TokenError::NotFound)?;
    if let Some(grants) = &edit.grants {
        check_targets(db, org_id, grants).await?;
    }
    let issuer = issuer(db, org_id, row.issuer_id).await?;
    let txn = db.begin().await?;
    let (after, revoked) = writes::patch(&txn, actor, account, &issuer, policy_id, &edit).await?;
    txn.commit().await?;
    invalidate(&revoked);
    dto(db, &after).await
}

pub(super) async fn delete(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    policy_id: Uuid,
) -> Result<(), TokenError> {
    let row = issuer_trust_policy::find(db, account.user_id, policy_id)
        .await?
        .ok_or(TokenError::NotFound)?;
    let issuer = issuer(db, account.org_id, row.issuer_id).await?;
    let txn = db.begin().await?;
    let revoked = writes::delete(&txn, actor, account, &issuer, policy_id).await?;
    txn.commit().await?;
    invalidate(&revoked);
    Ok(())
}

/// The transactional half of each write: it takes the transaction and only
/// the transaction, and decides from the row it locks.
mod writes {
    use super::*;

    /// The policy **of this account** as it is now, under its row lock.
    async fn locked(
        txn: &DatabaseTransaction,
        account: &service_accounts::Model,
        policy_id: Uuid,
    ) -> Result<oidc_issuer_trust_policies::Model, TokenError> {
        issuer_trust_policy::lock(txn, policy_id)
            .await?
            .filter(|policy| policy.service_account_id == account.user_id)
            .ok_or(TokenError::NotFound)
    }

    pub(super) async fn create(
        txn: &DatabaseTransaction,
        actor: &RequestActor,
        account: &service_accounts::Model,
        issuer: &oidc_issuers::Model,
        new: NewIssuerPolicy,
    ) -> Result<oidc_issuer_trust_policies::Model, TokenError> {
        let row = issuer_trust_policy::create(txn, new).await?;
        let grants = issuer_trust_policy::grants_for(txn, &[row.id]).await?;
        let detail = summary(account, &row, &issuer.issuer, &grants);
        audit::record_in_txn(txn, entry(actor, CREATED, &row, &issuer.name, detail)).await?;
        Ok(row)
    }

    pub(super) async fn patch(
        txn: &DatabaseTransaction,
        actor: &RequestActor,
        account: &service_accounts::Model,
        issuer: &oidc_issuers::Model,
        policy_id: Uuid,
        edit: &IssuerPolicyEdit,
    ) -> Result<(oidc_issuer_trust_policies::Model, Vec<api_tokens::Model>), TokenError> {
        let before = locked(txn, account, policy_id).await?;
        let grants_before = issuer_trust_policy::grants_for(txn, &[policy_id]).await?;
        let after = issuer_trust_policy::update(txn, before.clone(), edit).await?;
        let grants_after = issuer_trust_policy::grants_for(txn, &[policy_id]).await?;
        let was = summary(account, &before, &issuer.issuer, &grants_before);
        let now = summary(account, &after, &issuer.issuer, &grants_after);
        let mut revoked = Vec::new();
        if was != now {
            revoked = ci::revoke_for_policy(txn, policy_id, actor.id).await?;
            let detail = json!({ "revoked_tokens": revoked.len() });
            let row = entry(actor, UPDATED, &after, &issuer.name, detail).change(was, now);
            audit::record_in_txn(txn, row).await?;
        }
        Ok((after, revoked))
    }

    pub(super) async fn delete(
        txn: &DatabaseTransaction,
        actor: &RequestActor,
        account: &service_accounts::Model,
        issuer: &oidc_issuers::Model,
        policy_id: Uuid,
    ) -> Result<Vec<api_tokens::Model>, TokenError> {
        let row = locked(txn, account, policy_id).await?;
        let grants = issuer_trust_policy::grants_for(txn, &[policy_id]).await?;
        let revoked = ci::revoke_for_policy(txn, policy_id, actor.id).await?;
        issuer_trust_policy::delete(txn, policy_id).await?;
        let detail = json!({ "revoked_tokens": revoked.len() });
        let was = summary(account, &row, &issuer.issuer, &grants);
        let deleted = entry(actor, DELETED, &row, &issuer.name, detail).change(was, json!(null));
        audit::record_in_txn(txn, deleted).await?;
        Ok(revoked)
    }
}
