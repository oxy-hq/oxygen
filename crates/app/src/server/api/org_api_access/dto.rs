//! The wire shapes of the org's API access routes (tokens HTTP contract,
//! Phase 3), mapped from stored rows with no database.

use chrono::{DateTime, Utc};
use entity::{oidc_trust_policies, service_accounts};
use serde::Serialize;
use uuid::Uuid;

use crate::server::api::user_tokens::dto::{GrantDto, KIND_LEGACY, TokenDto};

/// Who created a service account. `label` is their address, or their name.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CreatorDto {
    pub id: Uuid,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ServiceAccountDto {
    /// The account's `users.id` — what its tokens act as.
    pub id: Uuid,
    pub org_id: Uuid,
    /// A slug, unique in the org.
    pub name: String,
    pub description: Option<String>,
    /// `member` or `admin`. Never `owner`.
    pub org_role: String,
    /// `null` once the creator's account is gone.
    pub created_by: Option<CreatorDto>,
    pub created_at: DateTime<Utc>,
    pub disabled_at: Option<DateTime<Utc>>,
    /// Tokens that are not revoked.
    pub token_count: u64,
    /// Trusted-access policies; none until Phase 4.
    pub trust_policy_count: u64,
}

pub(super) fn account_dto(
    row: &service_accounts::Model,
    created_by: Option<CreatorDto>,
    token_count: u64,
    trust_policy_count: u64,
) -> ServiceAccountDto {
    ServiceAccountDto {
        id: row.user_id,
        org_id: row.org_id,
        name: row.name.clone(),
        description: row.description.clone(),
        org_role: row.org_role.clone(),
        created_by,
        created_at: row.created_at.into(),
        disabled_at: row.disabled_at.map(Into::into),
        token_count,
        trust_policy_count,
    }
}

/// The wire shape of a trust policy (the contract's `TrustPolicy`).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TrustPolicyDto {
    pub id: Uuid,
    pub org_id: Uuid,
    pub service_account_id: Uuid,
    pub provider: String,
    /// `owner/repo` — display only; the ids are what a run is matched on.
    pub repository: String,
    pub repository_id: i64,
    pub repository_owner_id: i64,
    pub workflow_path: String,
    pub environment: Option<String>,
    pub ref_pattern: Option<String>,
    pub allow_self_hosted: bool,
    pub grants: Vec<GrantDto>,
    pub created_by: Option<CreatorDto>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub disabled_at: Option<DateTime<Utc>>,
}

impl TrustPolicyDto {
    pub(super) fn of(
        row: &oidc_trust_policies::Model,
        grants: Vec<GrantDto>,
        created_by: Option<CreatorDto>,
    ) -> Self {
        Self {
            id: row.id,
            org_id: row.org_id,
            service_account_id: row.service_account_id,
            provider: row.provider.clone(),
            repository: row.repository.clone(),
            repository_id: row.repository_id,
            repository_owner_id: row.repository_owner_id,
            workflow_path: row.workflow_path.clone(),
            environment: row.environment.clone(),
            ref_pattern: row.ref_pattern.clone(),
            allow_self_hosted: row.allow_self_hosted,
            grants,
            created_by,
            created_at: row.created_at.into(),
            last_used_at: row.last_used_at.map(Into::into),
            disabled_at: row.disabled_at.map(Into::into),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TrustPolicyList {
    pub trust_policies: Vec<TrustPolicyDto>,
}

#[derive(Debug, Serialize)]
pub struct ServiceAccountList {
    pub service_accounts: Vec<ServiceAccountDto>,
}

/// One row of the org token inventory: the token, and what it holds **here**.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct InventoryTokenDto {
    /// `grants` carries this org's grants only: what a person's token reaches
    /// in other orgs is not this org's to read. `blocked_orgs` likewise names
    /// this org alone, if its policy blocks the token.
    #[serde(flatten)]
    pub token: TokenDto,
    /// The token's grants in this org, the ones the org revoked included.
    /// Empty for a legacy key and for an all-access token the org has not
    /// blocked: they reach whatever their owner does.
    pub grants_here: Vec<GrantDto>,
    /// Why this org's token policy makes the token inert here —
    /// `max_lifetime` or `all_access_disallowed` — or `null`. Never set for a
    /// legacy key, which no policy binds.
    pub blocked_by_policy: Option<String>,
    /// A long-lived new-format token — no expiry, or more than
    /// [`LONG_LIVED_DAYS`] left — in an org whose CI already uses trusted
    /// access, so a stored secret may no longer be needed (design §5).
    pub long_lived_while_trusted_access: bool,
}

/// More time left than this makes a token "long-lived" for the inventory flag.
pub const LONG_LIVED_DAYS: i64 = 90;

#[derive(Debug, Serialize)]
pub struct OrgTokenList {
    pub tokens: Vec<InventoryTokenDto>,
}

/// What the inventory knows about the org it is listing for.
pub(super) struct Here {
    pub org_id: Uuid,
    /// The org has a live trust policy, on an account that is not disabled.
    pub trusted_access: bool,
    pub now: DateTime<Utc>,
}

/// Whether `token` is a long-lived stored secret: not a legacy key, and no
/// expiry or more than [`LONG_LIVED_DAYS`] left.
fn long_lived(token: &TokenDto, now: DateTime<Utc>) -> bool {
    token.kind != KIND_LEGACY
        && token
            .expires_at
            .is_none_or(|at| at - now > chrono::Duration::days(LONG_LIVED_DAYS))
}

/// A token with only this org's grants and policy block on it, and those same
/// grants beside it.
pub(super) fn inventory_dto(mut token: TokenDto, here: &Here) -> InventoryTokenDto {
    token.grants.retain(|g| g.org_id == here.org_id);
    token.blocked_orgs.retain(|b| b.org_id == here.org_id);
    let blocked_by_policy = token.blocked_orgs.first().map(|b| b.reason.clone());
    let long_lived_while_trusted_access = here.trusted_access && long_lived(&token, here.now);
    InventoryTokenDto {
        grants_here: token.grants.clone(),
        token,
        blocked_by_policy,
        long_lived_while_trusted_access,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::api::user_tokens::dto::{BlockedOrgDto, OwnerDto};

    const ORG: Uuid = Uuid::from_u128(0xA);
    const ELSEWHERE: Uuid = Uuid::from_u128(0xB);

    fn grant(org_id: Uuid) -> GrantDto {
        GrantDto {
            id: Uuid::new_v4(),
            kind: "workspace".into(),
            org_id,
            org_name: "org".into(),
            workspace_id: None,
            workspace_name: None,
            role_ceiling: Some("member".into()),
            app_id: None,
            app_name: None,
            org_slug: None,
            app_slug: None,
            revoked_at: None,
        }
    }

    fn token(grants: Vec<GrantDto>) -> TokenDto {
        TokenDto {
            id: Uuid::new_v4(),
            name: "laptop".into(),
            kind: "personal".into(),
            display_prefix: "oxy_pat_Ab3x".into(),
            last_four: "wxyz".into(),
            all_access: false,
            platform: false,
            partner: false,
            grants,
            expires_at: None,
            last_used_at: None,
            created_at: Utc::now(),
            revoked_at: None,
            status: "active",
            source: "ui".into(),
            owner: OwnerDto::user(Uuid::from_u128(1), "ada@acme.com"),
            blocked_orgs: Vec::new(),
        }
    }

    fn here(trusted_access: bool) -> Here {
        Here {
            org_id: ORG,
            trusted_access,
            now: Utc::now(),
        }
    }

    fn blocked(org_id: Uuid, reason: &str) -> BlockedOrgDto {
        BlockedOrgDto {
            org_id,
            org_name: "org".into(),
            reason: reason.into(),
        }
    }

    #[test]
    fn the_inventory_shows_only_this_orgs_grants() {
        // A person's token may reach other orgs: their names stay out of here.
        let row = inventory_dto(token(vec![grant(ORG), grant(ELSEWHERE)]), &here(false));
        assert_eq!(row.grants_here.len(), 1);
        assert_eq!(row.grants_here[0].org_id, ORG);
        assert_eq!(row.token.grants, row.grants_here);
        assert_eq!(row.blocked_by_policy, None);

        let json = serde_json::to_value(&row).unwrap();
        // Flattened: the token's own fields sit beside `grants_here`.
        assert_eq!(json["kind"], "personal");
        assert_eq!(json["owner"]["type"], "user");
        assert!(json["grants_here"].is_array());
        assert!(json["blocked_by_policy"].is_null());
        assert_eq!(json["long_lived_while_trusted_access"], false);
    }

    #[test]
    fn the_inventory_names_this_orgs_policy_block_only() {
        let mut t = token(vec![grant(ORG)]);
        t.blocked_orgs = vec![
            blocked(ELSEWHERE, "all_access_disallowed"),
            blocked(ORG, "max_lifetime"),
        ];
        let row = inventory_dto(t, &here(false));
        assert_eq!(row.blocked_by_policy.as_deref(), Some("max_lifetime"));
        assert_eq!(row.token.blocked_orgs, vec![blocked(ORG, "max_lifetime")]);

        let mut elsewhere = token(vec![grant(ORG)]);
        elsewhere.blocked_orgs = vec![blocked(ELSEWHERE, "max_lifetime")];
        let row = inventory_dto(elsewhere, &here(false));
        assert_eq!(
            row.blocked_by_policy, None,
            "another org's block is not ours"
        );
        assert!(row.token.blocked_orgs.is_empty());
    }

    #[test]
    fn a_long_lived_token_is_flagged_only_where_ci_uses_trusted_access() {
        let now = Utc::now();
        let forever = token(vec![grant(ORG)]);
        assert!(inventory_dto(forever.clone(), &here(true)).long_lived_while_trusted_access);
        assert!(!inventory_dto(forever, &here(false)).long_lived_while_trusted_access);

        let mut soon = token(vec![grant(ORG)]);
        soon.expires_at = Some(now + chrono::Duration::days(30));
        assert!(!inventory_dto(soon, &here(true)).long_lived_while_trusted_access);

        let mut far = token(vec![grant(ORG)]);
        far.expires_at = Some(now + chrono::Duration::days(LONG_LIVED_DAYS + 1));
        assert!(inventory_dto(far, &here(true)).long_lived_while_trusted_access);

        // A legacy key is never flagged: nothing here may act on it.
        let mut legacy = token(Vec::new());
        legacy.kind = KIND_LEGACY.into();
        assert!(!inventory_dto(legacy, &here(true)).long_lived_while_trusted_access);
    }

    #[test]
    fn a_service_account_reads_as_the_contract_says() {
        let now = Utc::now().fixed_offset();
        let row = service_accounts::Model {
            user_id: Uuid::from_u128(7),
            org_id: ORG,
            org_role: "admin".into(),
            name: "deploy-bot".into(),
            description: None,
            created_by: None,
            created_at: now,
            disabled_at: Some(now),
        };
        let json = serde_json::to_value(account_dto(&row, None, 2, 3)).unwrap();
        assert_eq!(json["id"], row.user_id.to_string());
        assert_eq!(json["org_role"], "admin");
        assert_eq!(json["token_count"], 2);
        assert_eq!(json["trust_policy_count"], 3);
        assert!(json["created_by"].is_null());
        assert!(json["disabled_at"].is_string());
    }
}
