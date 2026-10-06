//! Trust policies on a service account: list, register, edit, delete
//! (API-tokens design §3.4). The handlers are in [`super::handlers`]; this is
//! what they do, each write with its audit row in the same transaction.
//!
//! Registration resolves `owner/repo` to GitHub's numeric ids
//! ([`super::repo_resolve`]); the ids a body carries are used only when that
//! fails, and with neither the answer is 422 `repository_unresolved`.
//!
//! **Any change to a policy ends the tokens it minted.** A run holding a
//! 15-minute token minted under the old terms — a wider grant, a workflow the
//! policy no longer names — stops at the edit, not 15 minutes later. So does
//! one whose policy is disabled or deleted.

use std::collections::{HashMap, HashSet};

use entity::prelude::{Apps, Users};
use entity::{
    api_token_grants, api_tokens, apps, oidc_trust_policies, oidc_trust_policy_grants,
    service_accounts, users,
};
use oxy_app_core::audit::{AuditEntry, RequestActor};
use oxy_auth::token::service_account;
use oxy_auth::token::trust_policy::{self, NewPolicy};
use oxy_auth::token::trust_policy_access::{
    CreatePolicyBody, PatchPolicyBody, PolicyGrant, PolicyWant, RepoIds,
};
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
};
use serde_json::{Value, json};
use uuid::Uuid;

use super::account_tokens::check_workspaces;
use super::dto::{CreatorDto, TrustPolicyDto};
use super::repo_resolve;
use crate::server::api::user_tokens::dto::{GrantDto, grant_dto};
use crate::server::api::user_tokens::error::TokenError;
use crate::server::api::user_tokens::view;

const TARGET_TYPE: &str = "trust_policy";
const CREATED: &str = "trust_policy.created";
const UPDATED: &str = "trust_policy.updated";
const DELETED: &str = "trust_policy.deleted";

type GrantRow = oidc_trust_policy_grants::Model;

/// One policy of this account, or 404. For an answer that needs no lock: a
/// write decides from the row it locks (`writes`), never from this one.
async fn find<C: ConnectionTrait>(
    db: &C,
    account: &service_accounts::Model,
    policy_id: Uuid,
) -> Result<oidc_trust_policies::Model, TokenError> {
    trust_policy::find(db, account.user_id, policy_id)
        .await?
        .ok_or(TokenError::NotFound)
}

/// A policy grant in the shape a token grant has, so the two render through
/// one DTO and one name lookup.
fn as_token_grant(grant: &GrantRow) -> api_token_grants::Model {
    api_token_grants::Model {
        id: grant.id,
        token_id: grant.policy_id,
        kind: grant.kind.clone(),
        org_id: grant.org_id,
        workspace_id: grant.workspace_id,
        role_ceiling: grant.role_ceiling.clone(),
        app_id: grant.app_id,
        created_at: grant.created_at,
        revoked_at: None,
        revoked_by: None,
    }
}

/// The grants of these policies as DTOs, by policy.
pub(crate) async fn grant_dtos<C: ConnectionTrait>(
    db: &C,
    policy_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<GrantDto>>, TokenError> {
    let rows = trust_policy::grants_for(db, policy_ids).await?;
    let shaped: Vec<api_token_grants::Model> = rows.iter().map(as_token_grant).collect();
    let names = view::names_for(db, &shaped).await?;
    let mut out: HashMap<Uuid, Vec<GrantDto>> = HashMap::new();
    for grant in &shaped {
        out.entry(grant.token_id)
            .or_default()
            .push(grant_dto(grant, &names));
    }
    Ok(out)
}

async fn creators<C: ConnectionTrait>(
    db: &C,
    rows: &[oidc_trust_policies::Model],
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

async fn dtos<C: ConnectionTrait>(
    db: &C,
    rows: &[oidc_trust_policies::Model],
) -> Result<Vec<TrustPolicyDto>, TokenError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let mut grants = grant_dtos(db, &ids).await?;
    let creators = creators(db, rows).await?;
    Ok(rows
        .iter()
        .map(|row| {
            let creator = row.created_by.and_then(|id| creators.get(&id).cloned());
            TrustPolicyDto::of(row, grants.remove(&row.id).unwrap_or_default(), creator)
        })
        .collect())
}

async fn dto<C: ConnectionTrait>(
    db: &C,
    row: &oidc_trust_policies::Model,
) -> Result<TrustPolicyDto, TokenError> {
    let mut out = dtos(db, std::slice::from_ref(row)).await?;
    out.pop()
        .ok_or_else(|| TokenError::Internal("a trust policy mapped to nothing".into()))
}

pub(super) async fn list(
    db: &DatabaseConnection,
    account: &service_accounts::Model,
) -> Result<Vec<TrustPolicyDto>, TokenError> {
    let rows = trust_policy::list(db, account.user_id).await?;
    dtos(db, &rows).await
}

fn role_of(
    account: &service_accounts::Model,
) -> Result<oxy_auth::token::account_access::AccountRole, TokenError> {
    service_account::role_of(account)
        .ok_or_else(|| TokenError::Internal("service account has an unknown org_role".into()))
}

/// Every app a grant names must be this org's. Another org's reads the same
/// as none.
async fn check_apps<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    grants: &[PolicyGrant],
) -> Result<(), TokenError> {
    let wanted: HashSet<Uuid> = grants
        .iter()
        .filter_map(|g| match g {
            PolicyGrant::AppPublish { app_id, .. } => Some(*app_id),
            PolicyGrant::Workspace(_) => None,
        })
        .collect();
    if wanted.is_empty() {
        return Ok(());
    }
    let found = Apps::find()
        .filter(apps::Column::Id.is_in(wanted.iter().copied()))
        .filter(apps::Column::OrgId.eq(org_id))
        .all(db)
        .await?;
    if found.len() == wanted.len() {
        Ok(())
    } else {
        Err(TokenError::NotFound)
    }
}

/// Every workspace and app the grants name exists in the org.
async fn check_targets(
    db: &DatabaseConnection,
    org_id: Uuid,
    grants: &[PolicyGrant],
) -> Result<(), TokenError> {
    let workspace: Vec<_> = grants
        .iter()
        .filter_map(|g| match g {
            PolicyGrant::Workspace(spec) => Some(spec.clone()),
            PolicyGrant::AppPublish { .. } => None,
        })
        .collect();
    check_workspaces(db, org_id, &workspace).await?;
    check_apps(db, org_id, grants).await
}

/// The repository's ids and the name to display: GitHub's answer when it
/// gives one, else the ids the body carried, else 422.
async fn repository_of(
    db: &DatabaseConnection,
    org_id: Uuid,
    want: &PolicyWant,
) -> Result<(RepoIds, String), TokenError> {
    if let Some(found) = repo_resolve::resolve(db, org_id, &want.owner, &want.repo).await {
        return Ok((found.ids, found.full_name));
    }
    match want.explicit_ids {
        Some(ids) => Ok((ids, want.repository())),
        None => Err(TokenError::RepositoryUnresolved),
    }
}

/// The grant set, in an order that depends on what it grants and not on row
/// ids — so replacing a set with an equal one reads as no change.
fn grants_summary(grants: &[GrantRow]) -> Value {
    let mut summary: Vec<Value> = grants
        .iter()
        .map(|g| {
            json!({
                "kind": g.kind,
                "workspace_id": g.workspace_id,
                "role_ceiling": g.role_ceiling,
                "app_id": g.app_id,
            })
        })
        .collect();
    summary.sort_by_key(Value::to_string);
    Value::Array(summary)
}

fn summary(
    account: &service_accounts::Model,
    row: &oidc_trust_policies::Model,
    grants: &[GrantRow],
) -> Value {
    json!({
        "service_account_id": account.user_id,
        "service_account": account.name,
        "provider": row.provider,
        "repository": row.repository,
        "repository_id": row.repository_id,
        "repository_owner_id": row.repository_owner_id,
        "workflow_path": row.workflow_path,
        "environment": row.environment,
        "ref_pattern": row.ref_pattern,
        "allow_self_hosted": row.allow_self_hosted,
        "disabled": row.disabled_at.is_some(),
        "grants": grants_summary(grants),
    })
}

fn entry(
    actor: &RequestActor,
    action: &'static str,
    row: &oidc_trust_policies::Model,
    detail: Value,
) -> AuditEntry {
    let label = format!("{} {}", row.repository, row.workflow_path);
    AuditEntry::for_request(actor, action)
        .org(row.org_id)
        .target(TARGET_TYPE, row.id.to_string(), label)
        .metadata(detail)
}

fn invalidate(tokens: &[api_tokens::Model]) {
    for token in tokens {
        oxy_auth::token::cache::invalidate_token(token.id);
    }
}

// The three writes. Each handler does what needs the POOL — validation, a
// GitHub lookup, the answer's DTO — before `begin` or after `commit`, and hands
// the transaction to `writes`, whose functions take nothing else: a write
// holding a connection and a row lock must never wait on the pool for a
// second connection, or enough of them at once hold every one and release none.

pub(super) async fn create(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    body: CreatePolicyBody,
) -> Result<TrustPolicyDto, TokenError> {
    let org_id = account.org_id;
    let want = body.want(org_id, role_of(account)?)?;
    if want.environment.is_none() && trust_policy::environment_required(db, org_id).await? {
        return Err(TokenError::EnvironmentRequired);
    }
    check_targets(db, org_id, &want.grants).await?;
    let (ids, repository) = repository_of(db, org_id, &want).await?;
    let new = NewPolicy {
        org_id,
        service_account_id: account.user_id,
        repository,
        ids,
        workflow_path: want.workflow_path,
        environment: want.environment,
        ref_pattern: want.ref_pattern,
        allow_self_hosted: want.allow_self_hosted,
        grants: want.grants,
        created_by: actor.id,
    };

    let txn = db.begin().await?;
    let row = writes::create(&txn, actor, account, new).await?;
    txn.commit().await?;
    dto(db, &row).await
}

pub(super) async fn patch(
    db: &DatabaseConnection,
    actor: &RequestActor,
    account: &service_accounts::Model,
    policy_id: Uuid,
    body: PatchPolicyBody,
) -> Result<TrustPolicyDto, TokenError> {
    let org_id = account.org_id;
    let edit = body.edit(org_id, role_of(account)?)?;
    // The answers that need the pool, in the order they have always come: not
    // found, the environment requirement, the grants' targets. The first two
    // do not decide the edit — the policy and the requirement are read again
    // under the lock — and the targets depend only on the request and the org.
    find(db, account, policy_id).await?;
    let clears_environment = edit.environment == Some(None);
    if clears_environment && trust_policy::environment_required(db, org_id).await? {
        return Err(TokenError::EnvironmentRequired);
    }
    if let Some(grants) = &edit.grants {
        check_targets(db, org_id, grants).await?;
    }

    let txn = db.begin().await?;
    let (after, revoked) = writes::patch(&txn, actor, account, policy_id, &edit).await?;
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
    let txn = db.begin().await?;
    let revoked = writes::delete(&txn, actor, account, policy_id).await?;
    txn.commit().await?;
    invalidate(&revoked);
    Ok(())
}

#[path = "trust_policies_writes.rs"]
mod writes;

#[cfg(test)]
#[path = "trust_policies_tests.rs"]
mod tests;
