//! Trust policies: the writes and reads behind
//! `/api/orgs/{org_id}/service-accounts/{sa_id}/trust-policies`, and the
//! lookup the exchange matches against (API-tokens design §3.4).
//!
//! Database primitives only, like [`super::service_account`]: who may ask is
//! decided by the handler, which also writes the audit row in the same
//! transaction.
//!
//! A policy hangs off one service account and goes with it. Whether it can
//! mint is two rows' say: the policy's own `disabled_at`, and its account's.
//! Disabling an account therefore stops every policy on it without touching
//! one, and enabling it again brings them back as they were.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::prelude::{
    ApiTokens, OidcTrustPolicies, OidcTrustPolicyGrants, Organizations, ServiceAccounts,
};
use entity::{api_tokens, oidc_trust_policies, oidc_trust_policy_grants, service_accounts};
use oxy_shared::errors::OxyError;
use sea_orm::sea_query::Expr;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, Set,
};
use uuid::Uuid;

use super::grant_row::GrantRow;
use super::trust_policy_access::{PolicyEdit, PolicyGrant, RepoIds};

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// Whether a trust policy in this org must name an `environment`.
///
/// **Required unless the org's token policy says otherwise**
/// (`require_environment_on_trust_policies`, design §5; the default is
/// required). A policy with no environment lets anyone who can push to the
/// repository mint, because nothing then stands between a branch and the
/// workflow that runs on it. This is the one place the decision is made —
/// registration, every later edit, **and every exchange** ask here. The
/// exchange asking is what makes turning the requirement back on reach the
/// policies that already exist: one that names no environment stops matching
/// (`super::exchange::decide`) without being rewritten, and matches again if
/// the org relaxes it.
pub async fn environment_required<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
) -> Result<bool, OxyError> {
    Ok(super::policy_store::load(db, org_id)
        .await?
        .require_environment_on_trust_policies)
}

/// What to register.
#[derive(Clone, Debug)]
pub struct NewPolicy {
    pub org_id: Uuid,
    pub service_account_id: Uuid,
    /// `owner/repo`, for display.
    pub repository: String,
    pub ids: RepoIds,
    pub workflow_path: String,
    pub environment: Option<String>,
    pub ref_pattern: Option<String>,
    pub allow_self_hosted: bool,
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
            .for_policy(policy_id, now)
            .insert(db)
            .await
            .map_err(db_err("create trust policy grant"))?;
    }
    Ok(())
}

/// Register the policy and its grants. Run it in a transaction.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    new: NewPolicy,
) -> Result<oidc_trust_policies::Model, OxyError> {
    let id = Uuid::new_v4();
    let row = oidc_trust_policies::ActiveModel {
        id: Set(id),
        org_id: Set(new.org_id),
        service_account_id: Set(new.service_account_id),
        provider: Set(oidc_trust_policies::PROVIDER_GITHUB_ACTIONS.to_string()),
        repository_owner_id: Set(new.ids.repository_owner_id),
        repository_id: Set(new.ids.repository_id),
        repository: Set(new.repository),
        workflow_path: Set(new.workflow_path),
        environment: Set(new.environment),
        ref_pattern: Set(new.ref_pattern),
        allow_self_hosted: Set(new.allow_self_hosted),
        created_by: Set(Some(new.created_by)),
        created_at: Set(Utc::now().fixed_offset()),
        last_used_at: Set(None),
        disabled_at: Set(None),
    }
    .insert(db)
    .await
    .map_err(db_err("create trust policy"))?;
    insert_grants(db, id, &new.grants).await?;
    Ok(row)
}

/// The account's policies, oldest first.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    service_account_id: Uuid,
) -> Result<Vec<oidc_trust_policies::Model>, OxyError> {
    OidcTrustPolicies::find()
        .filter(oidc_trust_policies::Column::ServiceAccountId.eq(service_account_id))
        .order_by_asc(oidc_trust_policies::Column::CreatedAt)
        .order_by_asc(oidc_trust_policies::Column::Id)
        .all(db)
        .await
        .map_err(db_err("list trust policies"))
}

/// One policy **of this account**. Any other reads the same as none.
pub async fn find<C: ConnectionTrait>(
    db: &C,
    service_account_id: Uuid,
    policy_id: Uuid,
) -> Result<Option<oidc_trust_policies::Model>, OxyError> {
    Ok(OidcTrustPolicies::find_by_id(policy_id)
        .one(db)
        .await
        .map_err(db_err("trust policy lookup"))?
        .filter(|p| p.service_account_id == service_account_id))
}

/// Every grant row of these policies, oldest first.
pub async fn grants_for<C: ConnectionTrait>(
    db: &C,
    policy_ids: &[Uuid],
) -> Result<Vec<oidc_trust_policy_grants::Model>, OxyError> {
    if policy_ids.is_empty() {
        return Ok(Vec::new());
    }
    OidcTrustPolicyGrants::find()
        .filter(oidc_trust_policy_grants::Column::PolicyId.is_in(policy_ids.to_vec()))
        .order_by_asc(oidc_trust_policy_grants::Column::CreatedAt)
        .order_by_asc(oidc_trust_policy_grants::Column::Id)
        .all(db)
        .await
        .map_err(db_err("trust policy grants lookup"))
}

/// Apply an edit. `grants`, when given, replaces the whole set. Disabling
/// stamps `disabled_at` once; enabling clears it. Run it in a transaction.
pub async fn update<C: ConnectionTrait>(
    db: &C,
    row: oidc_trust_policies::Model,
    edit: &PolicyEdit,
) -> Result<oidc_trust_policies::Model, OxyError> {
    let id = row.id;
    let was_disabled = row.disabled_at.is_some();
    let mut active: oidc_trust_policies::ActiveModel = row.clone().into();
    if let Some(path) = &edit.workflow_path {
        active.workflow_path = Set(path.clone());
    }
    if let Some(environment) = &edit.environment {
        active.environment = Set(environment.clone());
    }
    if let Some(pattern) = &edit.ref_pattern {
        active.ref_pattern = Set(pattern.clone());
    }
    if let Some(allow) = edit.allow_self_hosted {
        active.allow_self_hosted = Set(allow);
    }
    match edit.disabled {
        Some(true) if !was_disabled => active.disabled_at = Set(Some(Utc::now().fixed_offset())),
        Some(false) => active.disabled_at = Set(None),
        _ => {}
    }
    let after = if active.is_changed() {
        active
            .update(db)
            .await
            .map_err(db_err("update trust policy"))?
    } else {
        row
    };
    if let Some(grants) = &edit.grants {
        OidcTrustPolicyGrants::delete_many()
            .filter(oidc_trust_policy_grants::Column::PolicyId.eq(id))
            .exec(db)
            .await
            .map_err(db_err("clear trust policy grants"))?;
        insert_grants(db, id, grants).await?;
    }
    Ok(after)
}

/// Delete the policy. Its grants go with it; the tokens it minted keep their
/// rows, with the link cleared here — `api_tokens.trust_policy_id` is a plain
/// uuid, not a foreign key, so nothing in the schema clears it.
pub async fn delete<C: ConnectionTrait>(db: &C, policy_id: Uuid) -> Result<(), OxyError> {
    ApiTokens::update_many()
        .col_expr(
            api_tokens::Column::TrustPolicyId,
            Expr::value(Option::<Uuid>::None),
        )
        .filter(api_tokens::Column::TrustPolicyId.eq(policy_id))
        .exec(db)
        .await
        .map_err(db_err("unlink a trust policy's tokens"))?;
    OidcTrustPolicies::delete_by_id(policy_id)
        .exec(db)
        .await
        .map_err(db_err("delete trust policy"))?;
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
    let rows = OidcTrustPolicies::find()
        .filter(oidc_trust_policies::Column::ServiceAccountId.is_in(account_ids.to_vec()))
        .all(db)
        .await
        .map_err(db_err("count trust policies"))?;
    let mut counts: HashMap<Uuid, u64> = HashMap::new();
    for row in rows {
        *counts.entry(row.service_account_id).or_default() += 1;
    }
    Ok(counts)
}

/// A policy that could mint, with the account it mints for.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub policy: oidc_trust_policies::Model,
    pub account: service_accounts::Model,
    pub org_slug: String,
}

impl Candidate {
    /// `<org_slug>/<name>`: the readable name the answer and the log carry.
    /// Never how a run names the account — that is its id, which cannot be
    /// re-pointed.
    pub fn account_name(&self) -> String {
        format!("{}/{}", self.org_slug, self.account.name)
    }
}

/// The **live** policies of one account on a repository, oldest first.
///
/// `account_id` is the account the run's workflow named, **by id**. Only that
/// account's own policies are read: **no other org's policy is loaded for the
/// request**, so one that another org registered on the same repository can
/// neither match this run nor change its answer.
///
/// By id, never by `<org_slug>/<name>`. A slug can be changed by its org's
/// owner and is free for anyone once the org renames or is deleted, and a name
/// is unique only within an org — so a workflow that said `acme/deployer`
/// would, after such a change, be naming whoever took the slug next. The id is
/// `service_accounts.user_id`: it never changes, and is never reused. The org's
/// slug is read here only to say, in the answer, where the run signed in.
///
/// An id that names no enabled account is no candidates, the same as an
/// account with no policy here — the two are answered alike.
///
/// Live: the policy is not disabled, and neither is its account. Narrowed by
/// both numeric ids, so the pure matcher sees only plausibly-relevant rows; it
/// re-checks every rule regardless.
pub async fn candidates<C: ConnectionTrait>(
    db: &C,
    account_id: Uuid,
    ids: RepoIds,
) -> Result<Vec<Candidate>, OxyError> {
    let account = ServiceAccounts::find_by_id(account_id)
        .filter(service_accounts::Column::DisabledAt.is_null())
        .one(db)
        .await
        .map_err(db_err("trust policy account"))?;
    let Some(account) = account else {
        return Ok(Vec::new());
    };
    let org = Organizations::find_by_id(account.org_id)
        .one(db)
        .await
        .map_err(db_err("trust policy org"))?;
    let Some(org) = org else {
        return Ok(Vec::new());
    };
    let policies = OidcTrustPolicies::find()
        .filter(oidc_trust_policies::Column::ServiceAccountId.eq(account.user_id))
        // A policy whose org differs from its account's is not one this
        // release wrote; it mints nothing.
        .filter(oidc_trust_policies::Column::OrgId.eq(account.org_id))
        .filter(oidc_trust_policies::Column::RepositoryId.eq(ids.repository_id))
        .filter(oidc_trust_policies::Column::RepositoryOwnerId.eq(ids.repository_owner_id))
        .filter(oidc_trust_policies::Column::DisabledAt.is_null())
        .order_by_asc(oidc_trust_policies::Column::CreatedAt)
        .order_by_asc(oidc_trust_policies::Column::Id)
        .all(db)
        .await
        .map_err(db_err("trust policy candidates"))?;
    Ok(policies
        .into_iter()
        .map(|policy| Candidate {
            policy,
            account: account.clone(),
            org_slug: org.slug.clone(),
        })
        .collect())
}

/// Stamp a policy as just used, and refresh the `owner/repo` it displays from
/// the run that matched — a repository renamed since registration shows its
/// current name.
pub async fn mark_used<C: ConnectionTrait>(
    db: &C,
    policy: oidc_trust_policies::Model,
    repository: &str,
    now: DateTime<Utc>,
) -> Result<(), OxyError> {
    let mut active: oidc_trust_policies::ActiveModel = policy.into();
    active.last_used_at = Set(Some(now.fixed_offset()));
    active.repository = Set(repository.to_string());
    active
        .update(db)
        .await
        .map_err(db_err("stamp trust policy use"))?;
    Ok(())
}
