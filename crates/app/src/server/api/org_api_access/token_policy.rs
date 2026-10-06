//! `GET|PUT /api/orgs/{org_id}/token-policy` — what the org asks of the API
//! tokens that reach it (API-tokens design §5, §8 Phase 5).
//!
//! An org with no row reads the defaults: no lifetime cap, all-access tokens
//! allowed, environments required on trust policies. A `PUT` replaces the
//! whole policy, and is audited as `token_policy.updated` with the policy
//! before and after, in the org's chain, in the transaction that writes it.
//!
//! A policy binds **new-format tokens only** and never revokes: a violating
//! token is inert in this org (`oxy_auth::token::policy`). After a write this
//! pod drops every cached credential, so the new policy holds here at once;
//! other pods follow within the cache's 30 s.

use entity::organizations;
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::token::policy::OrgPolicy;
use oxy_auth::token::policy_store;
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::server::api::user_tokens::error::TokenError;

const UPDATED: &str = "token_policy.updated";
const TARGET_TYPE: &str = "org_token_policy";

/// The wire shape, for both the read and the write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenPolicyDto {
    /// 1–3650, or `null` for no cap.
    #[serde(default)]
    pub max_lifetime_days: Option<i32>,
    pub allow_all_access_tokens: bool,
    pub require_environment_on_trust_policies: bool,
}

impl From<OrgPolicy> for TokenPolicyDto {
    fn from(p: OrgPolicy) -> Self {
        Self {
            max_lifetime_days: p.max_lifetime_days,
            allow_all_access_tokens: p.allow_all_access_tokens,
            require_environment_on_trust_policies: p.require_environment_on_trust_policies,
        }
    }
}

impl From<TokenPolicyDto> for OrgPolicy {
    fn from(d: TokenPolicyDto) -> Self {
        Self {
            max_lifetime_days: d.max_lifetime_days,
            allow_all_access_tokens: d.allow_all_access_tokens,
            require_environment_on_trust_policies: d.require_environment_on_trust_policies,
        }
    }
}

fn audit_value(p: &OrgPolicy) -> serde_json::Value {
    json!(TokenPolicyDto::from(*p))
}

pub(super) async fn get(
    db: &DatabaseConnection,
    org_id: uuid::Uuid,
) -> Result<TokenPolicyDto, TokenError> {
    Ok(policy_store::load(db, org_id).await?.into())
}

/// Replace the org's policy. Unchanged writes nothing and records nothing.
pub(super) async fn put(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org: &organizations::Model,
    body: TokenPolicyDto,
) -> Result<TokenPolicyDto, TokenError> {
    let after = OrgPolicy::from(body);
    after.validate().map_err(TokenError::Invalid)?;
    let before = policy_store::load(db, org.id).await?;
    if before == after {
        return Ok(after.into());
    }
    let txn = db.begin().await?;
    policy_store::save(&txn, org.id, &after, actor.id).await?;
    let entry = AuditEntry::for_request(actor, UPDATED)
        .org(org.id)
        .target(TARGET_TYPE, org.id.to_string(), org.name.clone())
        .change(audit_value(&before), audit_value(&after));
    audit::record_in_txn(&txn, entry).await?;
    txn.commit().await?;
    // The blocks ride cached credentials; drop them so this pod applies the
    // new policy now.
    oxy_auth::token::cache::invalidate_all();
    Ok(after.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_shape_round_trips_and_null_is_no_cap() {
        let body: TokenPolicyDto = serde_json::from_str(
            r#"{"max_lifetime_days":null,"allow_all_access_tokens":true,"require_environment_on_trust_policies":false}"#,
        )
        .unwrap();
        assert_eq!(body.max_lifetime_days, None);
        let policy = OrgPolicy::from(body);
        assert!(!policy.require_environment_on_trust_policies);
        assert_eq!(TokenPolicyDto::from(policy), body);
        assert_eq!(
            json!(TokenPolicyDto::from(OrgPolicy::default())),
            json!({
                "max_lifetime_days": null,
                "allow_all_access_tokens": true,
                "require_environment_on_trust_policies": true,
            })
        );
    }

    #[test]
    fn a_body_missing_a_flag_is_refused() {
        assert!(serde_json::from_str::<TokenPolicyDto>(r#"{"max_lifetime_days":30}"#).is_err());
        assert!(
            serde_json::from_str::<TokenPolicyDto>(
                r#"{"max_lifetime_days":"30","allow_all_access_tokens":true,"require_environment_on_trust_policies":true}"#
            )
            .is_err()
        );
    }
}
