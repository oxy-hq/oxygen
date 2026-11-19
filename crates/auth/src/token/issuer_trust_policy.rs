//! OIDC issuer trust policies: the writes and reads behind
//! `/api/orgs/{org_id}/service-accounts/{sa_id}/oidc-issuer-trust-policies`
//! and `/api/orgs/{org_id}/oidc-issuer-trust-policies`, and the lookup the
//! exchange matches against (API-tokens design §3.4, *Any OIDC issuer*).
//!
//! Database primitives only, like [`super::trust_policy`], which this
//! mirrors: a policy hangs off one Oxygen service account and one registered
//! issuer, and goes with either. Whether it can mint is two rows' say: the
//! policy's own `disabled_at`, and its account's.
//!
//! A policy's grants live in `oidc_issuer_trust_policy_grants`, the same
//! columns as a GitHub policy's. [`grants_for`] hands them back in
//! `oidc_trust_policy_grants`' shape, so capping them at a mint
//! ([`super::ci::capped_grants`]) and rendering them is one code path for
//! both kinds of policy.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::prelude::{
    ApiTokens, OidcIssuerTrustPolicies, OidcIssuerTrustPolicyGrants, Organizations, ServiceAccounts,
};
use entity::{
    api_tokens, oidc_issuer_trust_policies, oidc_issuer_trust_policy_grants, oidc_issuers,
    oidc_trust_policy_grants, service_accounts,
};
use oxy_shared::errors::OxyError;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set,
};
use uuid::Uuid;

use super::grant_row::GrantRow;
use super::issuer_trust_policy_access::IssuerPolicyEdit;
use super::trust_policy_access::PolicyGrant;
use crate::workload_identity::conditions::{self, Condition};

fn db_err(what: &'static str) -> impl FnOnce(DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// What to register.
#[derive(Clone, Debug)]
pub struct NewIssuerPolicy {
    pub org_id: Uuid,
    pub service_account_id: Uuid,
    pub issuer_id: Uuid,
    /// Already checked ([`conditions::check`]).
    pub conditions: Vec<Condition>,
    pub grants: Vec<PolicyGrant>,
    /// The person registering it — audit only.
    pub created_by: Uuid,
}

async fn insert_grants<C: ConnectionTrait>(
    db: &C,
    policy_id: Uuid,
    grants: &[PolicyGrant],
) -> Result<(), OxyError> {
    let now = Utc::now().fixed_offset();
    for grant in grants {
        GrantRow::of(grant)
            .for_issuer_policy(policy_id, now)
            .insert(db)
            .await
            .map_err(db_err("create oidc issuer trust policy grant"))?;
    }
    Ok(())
}

/// Register the policy and its grants. Run it in a transaction.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    new: NewIssuerPolicy,
) -> Result<oidc_issuer_trust_policies::Model, OxyError> {
    let id = Uuid::new_v4();
    let row = oidc_issuer_trust_policies::ActiveModel {
        id: Set(id),
        org_id: Set(new.org_id),
        service_account_id: Set(new.service_account_id),
        issuer_id: Set(new.issuer_id),
        conditions: Set(serde_json::to_value(&new.conditions)
            .map_err(|e| OxyError::DBError(format!("trust policy conditions: {e}")))?),
        created_by: Set(Some(new.created_by)),
        created_at: Set(Utc::now().fixed_offset()),
        last_used_at: Set(None),
        disabled_at: Set(None),
    }
    .insert(db)
    .await
    .map_err(db_err("create oidc issuer trust policy"))?;
    insert_grants(db, id, &new.grants).await?;
    Ok(row)
}

/// The account's policies, oldest first.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    service_account_id: Uuid,
) -> Result<Vec<oidc_issuer_trust_policies::Model>, OxyError> {
    OidcIssuerTrustPolicies::find()
        .filter(oidc_issuer_trust_policies::Column::ServiceAccountId.eq(service_account_id))
        .order_by_asc(oidc_issuer_trust_policies::Column::CreatedAt)
        .order_by_asc(oidc_issuer_trust_policies::Column::Id)
        .all(db)
        .await
        .map_err(db_err("list oidc issuer trust policies"))
}

/// Every policy in the org, of every account, oldest first: what the settings
/// screen lists under each issuer.
pub async fn list_org<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
) -> Result<Vec<oidc_issuer_trust_policies::Model>, OxyError> {
    OidcIssuerTrustPolicies::find()
        .filter(oidc_issuer_trust_policies::Column::OrgId.eq(org_id))
        .order_by_asc(oidc_issuer_trust_policies::Column::CreatedAt)
        .order_by_asc(oidc_issuer_trust_policies::Column::Id)
        .all(db)
        .await
        .map_err(db_err("list the org's oidc issuer trust policies"))
}

/// Every policy on one issuer, of any account: what deleting the issuer ends.
pub async fn on_issuer<C: ConnectionTrait>(
    db: &C,
    issuer_id: Uuid,
) -> Result<Vec<oidc_issuer_trust_policies::Model>, OxyError> {
    OidcIssuerTrustPolicies::find()
        .filter(oidc_issuer_trust_policies::Column::IssuerId.eq(issuer_id))
        .order_by_asc(oidc_issuer_trust_policies::Column::Id)
        .all(db)
        .await
        .map_err(db_err("oidc issuer trust policies on an issuer"))
}

/// How many policies each issuer has.
pub async fn counts_by_issuer<C: ConnectionTrait>(
    db: &C,
    issuer_ids: &[Uuid],
) -> Result<HashMap<Uuid, u64>, OxyError> {
    if issuer_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = OidcIssuerTrustPolicies::find()
        .filter(oidc_issuer_trust_policies::Column::IssuerId.is_in(issuer_ids.to_vec()))
        .all(db)
        .await
        .map_err(db_err("count oidc issuer trust policies by issuer"))?;
    let mut counts: HashMap<Uuid, u64> = HashMap::new();
    for row in rows {
        *counts.entry(row.issuer_id).or_default() += 1;
    }
    Ok(counts)
}

/// One policy **of this account**. Any other reads the same as none.
pub async fn find<C: ConnectionTrait>(
    db: &C,
    service_account_id: Uuid,
    policy_id: Uuid,
) -> Result<Option<oidc_issuer_trust_policies::Model>, OxyError> {
    Ok(OidcIssuerTrustPolicies::find_by_id(policy_id)
        .one(db)
        .await
        .map_err(db_err("oidc issuer trust policy lookup"))?
        .filter(|p| p.service_account_id == service_account_id))
}

/// An issuer policy's grant in a GitHub policy's grant shape: the same
/// columns, so one capping and one rendering serve both.
fn as_policy_grant(row: oidc_issuer_trust_policy_grants::Model) -> oidc_trust_policy_grants::Model {
    oidc_trust_policy_grants::Model {
        id: row.id,
        policy_id: row.policy_id,
        kind: row.kind,
        org_id: row.org_id,
        workspace_id: row.workspace_id,
        role_ceiling: row.role_ceiling,
        app_id: row.app_id,
        created_at: row.created_at,
    }
}

/// Every grant row of these policies, oldest first.
pub async fn grants_for<C: ConnectionTrait>(
    db: &C,
    policy_ids: &[Uuid],
) -> Result<Vec<oidc_trust_policy_grants::Model>, OxyError> {
    if policy_ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = OidcIssuerTrustPolicyGrants::find()
        .filter(oidc_issuer_trust_policy_grants::Column::PolicyId.is_in(policy_ids.to_vec()))
        .order_by_asc(oidc_issuer_trust_policy_grants::Column::CreatedAt)
        .order_by_asc(oidc_issuer_trust_policy_grants::Column::Id)
        .all(db)
        .await
        .map_err(db_err("oidc issuer trust policy grants lookup"))?;
    Ok(rows.into_iter().map(as_policy_grant).collect())
}

/// Apply an edit. `grants`, when given, replaces the whole set. Disabling
/// stamps `disabled_at` once; enabling clears it. Run it in a transaction,
/// on the row [`lock`] handed back.
pub async fn update<C: ConnectionTrait>(
    db: &C,
    row: oidc_issuer_trust_policies::Model,
    edit: &IssuerPolicyEdit,
) -> Result<oidc_issuer_trust_policies::Model, OxyError> {
    let id = row.id;
    let was_disabled = row.disabled_at.is_some();
    let mut active: oidc_issuer_trust_policies::ActiveModel = row.clone().into();
    match edit.disabled {
        Some(true) if !was_disabled => active.disabled_at = Set(Some(Utc::now().fixed_offset())),
        Some(false) => active.disabled_at = Set(None),
        _ => {}
    }
    let after = if active.is_changed() {
        active
            .update(db)
            .await
            .map_err(db_err("update oidc issuer trust policy"))?
    } else {
        row
    };
    if let Some(grants) = &edit.grants {
        OidcIssuerTrustPolicyGrants::delete_many()
            .filter(oidc_issuer_trust_policy_grants::Column::PolicyId.eq(id))
            .exec(db)
            .await
            .map_err(db_err("clear oidc issuer trust policy grants"))?;
        insert_grants(db, id, grants).await?;
    }
    Ok(after)
}

/// Unlink the tokens these policies minted. `api_tokens.trust_policy_id` is a
/// plain uuid, not a foreign key, so nothing in the schema clears it.
pub async fn unlink_tokens<C: ConnectionTrait>(
    db: &C,
    policy_ids: &[Uuid],
) -> Result<(), OxyError> {
    if policy_ids.is_empty() {
        return Ok(());
    }
    ApiTokens::update_many()
        .col_expr(
            api_tokens::Column::TrustPolicyId,
            Expr::value(Option::<Uuid>::None),
        )
        .filter(api_tokens::Column::TrustPolicyId.is_in(policy_ids.to_vec()))
        .exec(db)
        .await
        .map_err(db_err("unlink a oidc issuer trust policy's tokens"))?;
    Ok(())
}

/// Delete the policy. Its grants go with it; the tokens it minted keep their
/// rows, unlinked.
pub async fn delete<C: ConnectionTrait>(db: &C, policy_id: Uuid) -> Result<(), OxyError> {
    unlink_tokens(db, &[policy_id]).await?;
    OidcIssuerTrustPolicies::delete_by_id(policy_id)
        .exec(db)
        .await
        .map_err(db_err("delete oidc issuer trust policy"))?;
    Ok(())
}

/// How many policies each account has, disabled ones included.
pub async fn counts<C: ConnectionTrait>(
    db: &C,
    account_ids: &[Uuid],
) -> Result<HashMap<Uuid, u64>, OxyError> {
    if account_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = OidcIssuerTrustPolicies::find()
        .filter(oidc_issuer_trust_policies::Column::ServiceAccountId.is_in(account_ids.to_vec()))
        .all(db)
        .await
        .map_err(db_err("count oidc issuer trust policies"))?;
    let mut counts: HashMap<Uuid, u64> = HashMap::new();
    for row in rows {
        *counts.entry(row.service_account_id).or_default() += 1;
    }
    Ok(counts)
}

/// A policy that could mint, with the issuer it trusts and the account it
/// mints for.
#[derive(Clone, Debug)]
pub struct IssuerCandidate {
    pub policy: oidc_issuer_trust_policies::Model,
    pub issuer: oidc_issuers::Model,
    pub account: service_accounts::Model,
    pub org_slug: String,
}

impl IssuerCandidate {
    /// `<org_slug>/<name>`: the readable name the answer and the log carry.
    pub fn account_name(&self) -> String {
        format!("{}/{}", self.org_slug, self.account.name)
    }

    /// The policy's claim conditions.
    pub fn conditions(&self) -> Vec<Condition> {
        conditions_of(&self.policy)
    }
}

/// The claim conditions of a stored policy. One that does not read as a list
/// reads as none, which matches nothing.
pub fn conditions_of(policy: &oidc_issuer_trust_policies::Model) -> Vec<Condition> {
    conditions::stored(&policy.conditions)
}

/// The named account, **enabled**, with its org's slug. `None` for an id
/// that names no enabled account — which the exchange answers exactly as an
/// account with no policy.
pub async fn live_account<C: ConnectionTrait>(
    db: &C,
    account_id: Uuid,
) -> Result<Option<(service_accounts::Model, String)>, OxyError> {
    let account = ServiceAccounts::find_by_id(account_id)
        .filter(service_accounts::Column::DisabledAt.is_null())
        .one(db)
        .await
        .map_err(db_err("oidc issuer trust policy account"))?;
    let Some(account) = account else {
        return Ok(None);
    };
    let org = Organizations::find_by_id(account.org_id)
        .one(db)
        .await
        .map_err(db_err("oidc issuer trust policy org"))?;
    Ok(org.map(|org| (account, org.slug)))
}

/// The **live** policies of `account` on `issuer`, oldest first. Only that
/// account's own policies are read, as on the GitHub path.
pub async fn candidates<C: ConnectionTrait>(
    db: &C,
    account: &service_accounts::Model,
    org_slug: &str,
    issuer: &oidc_issuers::Model,
) -> Result<Vec<IssuerCandidate>, OxyError> {
    if issuer.org_id != account.org_id {
        return Ok(Vec::new());
    }
    let policies = OidcIssuerTrustPolicies::find()
        .filter(oidc_issuer_trust_policies::Column::ServiceAccountId.eq(account.user_id))
        .filter(oidc_issuer_trust_policies::Column::OrgId.eq(account.org_id))
        .filter(oidc_issuer_trust_policies::Column::IssuerId.eq(issuer.id))
        .filter(oidc_issuer_trust_policies::Column::DisabledAt.is_null())
        .order_by_asc(oidc_issuer_trust_policies::Column::CreatedAt)
        .order_by_asc(oidc_issuer_trust_policies::Column::Id)
        .all(db)
        .await
        .map_err(db_err("oidc issuer trust policy candidates"))?;
    Ok(policies
        .into_iter()
        .map(|policy| IssuerCandidate {
            policy,
            issuer: issuer.clone(),
            account: account.clone(),
            org_slug: org_slug.to_string(),
        })
        .collect())
}

/// Take the policy's row lock and hand back the row as it is now — what a
/// change to the policy does first, and what a mint takes second (after the
/// account's). As [`super::ci_mint::lock_policy`] is for a GitHub policy.
#[must_use = "decide from the row this returns: it is the policy as it is under the lock"]
pub async fn lock<C: ConnectionTrait>(
    db: &C,
    policy_id: Uuid,
) -> Result<Option<oidc_issuer_trust_policies::Model>, OxyError> {
    OidcIssuerTrustPolicies::find_by_id(policy_id)
        .lock_exclusive()
        .one(db)
        .await
        .map_err(db_err("lock oidc issuer trust policy"))
}

/// Stamp a policy as just used.
pub async fn mark_used<C: ConnectionTrait>(
    db: &C,
    policy: oidc_issuer_trust_policies::Model,
    now: DateTime<Utc>,
) -> Result<(), OxyError> {
    let mut active: oidc_issuer_trust_policies::ActiveModel = policy.into();
    active.last_used_at = Set(Some(now.fixed_offset()));
    active
        .update(db)
        .await
        .map_err(db_err("stamp oidc issuer trust policy use"))?;
    Ok(())
}
