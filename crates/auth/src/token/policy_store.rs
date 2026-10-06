//! The database side of the org token policy: read and write
//! `org_token_policies`, and work out which orgs a token's policy blocks it in.
//!
//! **On the request path this is read on a cache miss only.** The orgs a
//! token is blocked in ride its `CredentialContext.blocked_orgs`, which the
//! ≤30 s credential cache holds (`super::cache`); a `PUT` clears this pod's
//! cache, and other pods follow within the TTL. A policy cannot change with
//! the clock — a lifetime is `expires_at - created_at` — so a cached answer is
//! never stale for any reason but an edit.
//!
//! The orgs a token reaches, for this purpose:
//!
//! - a grant-bound token (`personal` with `all_access = false`,
//!   `service_account`, `ci`): the orgs of its live grants;
//! - an all-access personal token: every org its owner is a member of.
//!   Membership, not standing: an org's policy is about the tokens of the
//!   people in it. Staff or partner reach into another org is not judged.
//!
//! A legacy key reaches nothing here: it is never judged (§3.5), and nothing
//! about it is looked up, so no policy read can stand between it and a
//! request.

use std::collections::HashMap;

use chrono::Utc;
use entity::prelude::{OrgMembers, OrgTokenPolicies};
use entity::{api_token_grants, api_tokens, org_members, org_token_policies};
use oxy_shared::errors::OxyError;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter, Set};
use uuid::Uuid;

use super::policy::{OrgPolicy, TokenShape, Violation, blocked_in};

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// The org's policy: its row, or the defaults.
pub async fn load<C: ConnectionTrait>(db: &C, org_id: Uuid) -> Result<OrgPolicy, OxyError> {
    let row = OrgTokenPolicies::find_by_id(org_id)
        .one(db)
        .await
        .map_err(db_err("org token policy lookup"))?;
    Ok(OrgPolicy::of(row.as_ref()))
}

/// The policies among `org_ids` that can block a token. An org missing from
/// the map holds the defaults, which block nothing.
pub async fn restrictive<C: ConnectionTrait>(
    db: &C,
    org_ids: &[Uuid],
) -> Result<HashMap<Uuid, OrgPolicy>, OxyError> {
    if org_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = OrgTokenPolicies::find()
        .filter(org_token_policies::Column::OrgId.is_in(org_ids.to_vec()))
        .filter(
            Condition::any()
                .add(org_token_policies::Column::MaxLifetimeDays.is_not_null())
                .add(org_token_policies::Column::AllowAllAccessTokens.eq(false)),
        )
        .all(db)
        .await
        .map_err(db_err("org token policies lookup"))?;
    Ok(rows
        .iter()
        .map(|r| (r.org_id, OrgPolicy::of(Some(r))))
        .collect())
}

/// Write the org's policy, replacing any before it.
pub async fn save<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    policy: &OrgPolicy,
    by: Uuid,
) -> Result<(), OxyError> {
    let row = org_token_policies::ActiveModel {
        org_id: Set(org_id),
        max_lifetime_days: Set(policy.max_lifetime_days),
        allow_all_access_tokens: Set(policy.allow_all_access_tokens),
        require_environment_on_trust_policies: Set(policy.require_environment_on_trust_policies),
        updated_by: Set(Some(by)),
        updated_at: Set(Utc::now().fixed_offset()),
    };
    OrgTokenPolicies::insert(row)
        .on_conflict(
            OnConflict::column(org_token_policies::Column::OrgId)
                .update_columns([
                    org_token_policies::Column::MaxLifetimeDays,
                    org_token_policies::Column::AllowAllAccessTokens,
                    org_token_policies::Column::RequireEnvironmentOnTrustPolicies,
                    org_token_policies::Column::UpdatedBy,
                    org_token_policies::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec(db)
        .await
        .map_err(db_err("save org token policy"))?;
    Ok(())
}

/// The distinct orgs of the live grants among `grants`.
pub fn grant_orgs(grants: &[api_token_grants::Model]) -> Vec<Uuid> {
    let mut orgs: Vec<Uuid> = grants
        .iter()
        .filter(|g| g.revoked_at.is_none())
        .map(|g| g.org_id)
        .collect();
    orgs.sort_unstable();
    orgs.dedup();
    orgs
}

/// Every org each of `users` is a member of.
pub async fn member_orgs<C: ConnectionTrait>(
    db: &C,
    users: &[Uuid],
) -> Result<HashMap<Uuid, Vec<Uuid>>, OxyError> {
    if users.is_empty() {
        return Ok(HashMap::new());
    }
    let rows = OrgMembers::find()
        .filter(org_members::Column::UserId.is_in(users.to_vec()))
        .all(db)
        .await
        .map_err(db_err("org membership lookup"))?;
    let mut out: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
    for row in rows {
        out.entry(row.user_id).or_default().push(row.org_id);
    }
    Ok(out)
}

/// The orgs whose policy makes this token inert, with why. `grants` are the
/// token's rows, revoked or not. Empty for a legacy key, without a read.
pub async fn blocks<C: ConnectionTrait>(
    db: &C,
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
) -> Result<Vec<(Uuid, Violation)>, OxyError> {
    let Some(shape) = TokenShape::of(row).filter(|s| !s.legacy) else {
        return Ok(Vec::new());
    };
    let reach = if shape.all_access_personal() {
        member_orgs(db, &[row.principal_user_id])
            .await?
            .remove(&row.principal_user_id)
            .unwrap_or_default()
    } else {
        grant_orgs(grants)
    };
    let policies = restrictive(db, &reach).await?;
    Ok(blocked_in(&shape, &reach, &policies))
}

/// The tightest lifetime cap among `org_ids` — the orgs a grant-bound token
/// holds grants in, which its new expiry is checked against.
pub async fn tightest_cap_in<C: ConnectionTrait>(
    db: &C,
    org_ids: &[Uuid],
) -> Result<Option<i32>, OxyError> {
    let policies = restrictive(db, org_ids).await?;
    Ok(super::policy::tightest_cap(policies.values()))
}
