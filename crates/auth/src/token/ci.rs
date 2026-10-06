//! `oxy_ci_` tokens: what a trust policy mints for one CI run (API-tokens
//! design §3.4).
//!
//! A `ci` token acts as the policy's service account, for
//! [`LIFETIME_MINUTES`]. It is grant-bound, never all-access, with neither
//! standing flag — exactly an account's own token — and its grants are the
//! policy's, **capped by the account's standing now**: an account lowered to
//! Member since the policy was written mints nothing above Member.
//!
//! The row records the policy and the verified claims of the run, so "which
//! workflow run held this token" is one row read.

use chrono::{DateTime, Duration, Utc};
use entity::prelude::{ApiTokenGrants, ApiTokens};
use entity::{api_token_grants, api_tokens, oidc_trust_policy_grants};
use oxy_authz::RoleCeiling;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set,
};
use serde_json::Value;
use uuid::Uuid;

use super::account_access::AccountRole;
use super::credential::{StoredKind, source};
use super::grant_row::GrantRow;
use super::personal::{self, GrantSpec, Minted};
use super::trust_policy_access::PolicyGrant;

/// How long a minted token lives. Short: it exists to carry one job.
pub const LIFETIME_MINUTES: i64 = 15;

/// How long an expired `ci` row is kept before the sweep deletes it. Long
/// enough that a run's token is still there when its audit trail is read
/// (audit rows themselves keep 30 days).
pub const KEEP_EXPIRED_DAYS: i64 = 30;

/// Why a policy's tokens are revoked with it.
pub const REVOKED_WITH_POLICY: &str = "trust policy disabled or deleted";

fn db_err(what: &'static str) -> impl FnOnce(DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// A policy's stored grants, capped by the account's standing: a workspace
/// ceiling above the account's `org_role` is lowered to it. A row this release
/// cannot read — an unknown kind or ceiling, an `app_publish` naming no app —
/// grants nothing.
pub fn capped_grants(
    rows: &[oidc_trust_policy_grants::Model],
    role: AccountRole,
) -> Vec<PolicyGrant> {
    rows.iter()
        .filter_map(|row| match row.kind.as_str() {
            api_token_grants::KIND_WORKSPACE => {
                let ceiling = RoleCeiling::parse(row.role_ceiling.as_deref()?)?;
                Some(PolicyGrant::Workspace(GrantSpec {
                    org_id: row.org_id,
                    workspace_id: row.workspace_id,
                    ceiling: ceiling.min(role.ceiling()),
                }))
            }
            api_token_grants::KIND_APP_PUBLISH => Some(PolicyGrant::AppPublish {
                org_id: row.org_id,
                app_id: row.app_id?,
            }),
            _ => None,
        })
        .collect()
}

/// What to mint.
#[derive(Clone, Debug)]
pub struct NewCiToken {
    /// The service account the token acts as.
    pub account_id: Uuid,
    pub policy_id: Uuid,
    /// The verified workflow identity — what a publish records as
    /// `published_via`.
    pub name: String,
    pub grants: Vec<PolicyGrant>,
    /// The verified claims of the run. Never the JWT.
    pub claims: Value,
    pub now: DateTime<Utc>,
}

async fn insert_grants<C: ConnectionTrait>(
    db: &C,
    token_id: Uuid,
    grants: &[PolicyGrant],
    now: DateTime<Utc>,
) -> Result<(), OxyError> {
    for grant in grants {
        GrantRow::of(grant)
            .for_token(token_id, now.fixed_offset())
            .insert(db)
            .await
            .map_err(db_err("create ci token grant"))?;
    }
    Ok(())
}

/// Mint an `oxy_ci_` token and store its hash, its grants, the policy and the
/// run's claims. Run it in a transaction.
///
/// `created_by` is left NULL: no person minted it. The policy and the claims
/// say who did.
pub async fn mint<C: ConnectionTrait>(db: &C, new: NewCiToken) -> Result<Minted, OxyError> {
    let kind = StoredKind::Ci;
    let token = personal::generate_for(kind.as_str());
    let id = Uuid::new_v4();
    let expires_at = new.now + Duration::minutes(LIFETIME_MINUTES);
    let row = api_tokens::ActiveModel {
        id: Set(id),
        kind: Set(kind.as_str().to_string()),
        principal_user_id: Set(new.account_id),
        name: Set(new.name),
        display_prefix: Set(token.display_prefix),
        last_four: Set(token.last_four),
        token_hash: Set(token.token_hash),
        all_access: Set(false),
        platform: Set(false),
        partner: Set(false),
        expires_at: Set(Some(expires_at.fixed_offset())),
        last_used_at: Set(None),
        created_at: Set(new.now.fixed_offset()),
        created_by: Set(None),
        revoked_at: Set(None),
        revoked_by: Set(None),
        revoke_reason: Set(None),
        source: Set(source::OIDC.to_string()),
        legacy_api_key_id: Set(None),
        trust_policy_id: Set(Some(new.policy_id)),
        oidc_claims: Set(Some(new.claims)),
    }
    .insert(db)
    .await
    .map_err(db_err("create ci token"))?;
    insert_grants(db, id, &new.grants, new.now).await?;
    Ok(Minted {
        row,
        secret: token.plaintext,
    })
}

/// Revoke every live token a policy minted — when it is disabled or deleted,
/// so the run that holds one stops with it rather than 15 minutes later.
/// Returns the rows this call revoked, for the credential cache.
pub async fn revoke_for_policy<C: ConnectionTrait>(
    db: &C,
    policy_id: Uuid,
    by: Uuid,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    let live = ApiTokens::find()
        .filter(api_tokens::Column::TrustPolicyId.eq(policy_id))
        .filter(api_tokens::Column::Kind.eq(StoredKind::Ci.as_str()))
        .filter(api_tokens::Column::RevokedAt.is_null())
        .all(db)
        .await
        .map_err(db_err("list a trust policy's tokens"))?;
    let mut revoked = Vec::new();
    for token in live {
        if let Some(token) = personal::revoke(db, token, by, REVOKED_WITH_POLICY).await? {
            revoked.push(token);
        }
    }
    Ok(revoked)
}

/// Delete `ci` rows that expired more than [`KEEP_EXPIRED_DAYS`] ago. Their
/// grants and usage rows go with them. Idempotent, and confined to `ci`: no
/// other kind is ever deleted here — least of all a legacy key.
pub async fn delete_expired<C: ConnectionTrait>(db: &C, now: DateTime<Utc>) -> Result<u64, DbErr> {
    let cutoff = (now - Duration::days(KEEP_EXPIRED_DAYS)).fixed_offset();
    let expired: Vec<Uuid> = ApiTokens::find()
        .filter(api_tokens::Column::Kind.eq(StoredKind::Ci.as_str()))
        .filter(api_tokens::Column::ExpiresAt.lt(cutoff))
        .all(db)
        .await?
        .into_iter()
        .map(|t| t.id)
        .collect();
    if expired.is_empty() {
        return Ok(0);
    }
    // Explicit, though the foreign key cascades: the sweep states what it removes.
    ApiTokenGrants::delete_many()
        .filter(api_token_grants::Column::TokenId.is_in(expired.clone()))
        .exec(db)
        .await?;
    Ok(ApiTokens::delete_many()
        .filter(api_tokens::Column::Id.is_in(expired))
        .filter(api_tokens::Column::Kind.eq(StoredKind::Ci.as_str()))
        .exec(db)
        .await?
        .rows_affected)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG: Uuid = Uuid::from_u128(0xA);

    fn row(
        kind: &str,
        ceiling: Option<&str>,
        app_id: Option<Uuid>,
    ) -> oidc_trust_policy_grants::Model {
        oidc_trust_policy_grants::Model {
            id: Uuid::new_v4(),
            policy_id: Uuid::from_u128(1),
            kind: kind.into(),
            org_id: ORG,
            workspace_id: None,
            role_ceiling: ceiling.map(str::to_string),
            app_id,
            created_at: Utc::now().fixed_offset(),
        }
    }

    fn ceilings(grants: &[PolicyGrant]) -> Vec<RoleCeiling> {
        grants
            .iter()
            .filter_map(|g| match g {
                PolicyGrant::Workspace(spec) => Some(spec.ceiling),
                PolicyGrant::AppPublish { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_grant_is_capped_at_the_accounts_standing_now() {
        // Written while the account was an admin; the account is a member now.
        let rows = [row("workspace", Some("admin"), None)];
        assert_eq!(
            ceilings(&capped_grants(&rows, AccountRole::Member)),
            vec![RoleCeiling::Member]
        );
        assert_eq!(
            ceilings(&capped_grants(&rows, AccountRole::Admin)),
            vec![RoleCeiling::Admin]
        );
        // A grant below the standing is left where the policy put it.
        let rows = [row("workspace", Some("viewer"), None)];
        assert_eq!(
            ceilings(&capped_grants(&rows, AccountRole::Admin)),
            vec![RoleCeiling::Viewer]
        );
    }

    #[test]
    fn an_owner_ceiling_never_survives_the_cap() {
        let rows = [row("workspace", Some("owner"), None)];
        assert_eq!(
            ceilings(&capped_grants(&rows, AccountRole::Admin)),
            vec![RoleCeiling::Admin]
        );
    }

    #[test]
    fn a_row_that_cannot_be_read_grants_nothing() {
        let rows = [
            row("workspace", Some("superuser"), None),
            row("workspace", None, None),
            row("app_publish", None, None),
            row("everything", Some("admin"), None),
        ];
        assert!(capped_grants(&rows, AccountRole::Admin).is_empty());
    }

    #[test]
    fn an_app_publish_grant_is_carried_as_it_is() {
        let app = Uuid::from_u128(7);
        let rows = [row("app_publish", None, Some(app))];
        assert_eq!(
            capped_grants(&rows, AccountRole::Member),
            vec![PolicyGrant::AppPublish {
                org_id: ORG,
                app_id: app
            }]
        );
    }
}
