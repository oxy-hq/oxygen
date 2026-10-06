//! `GET /api/orgs/{org_id}/tokens` — every token that reaches the org, for its
//! admins (API-tokens design §5), and one such token's activity here.
//!
//! A token is listed when it is not revoked and one of these holds:
//!
//! - it is a **service-account** token of one of the org's accounts;
//! - it is **all-access** — every legacy key is — and its owner is a member;
//! - it holds a **grant in this org**, and either its owner can still act here
//!   (a member, or a token carrying staff or partner standing) or the org has
//!   already ended that reach, in which case the row is history: its
//!   `grants_here` are all revoked.
//!
//! What a grant covers is asked of the same [`TokenReach`] the request path
//! decides with, so the list cannot disagree with enforcement.
//!
//! Read-only, and it never shows more than the org is owed: a person's token
//! appears with this org's grants only (`dto::inventory_dto`), and its
//! activity with this org's events only.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use entity::prelude::{
    ApiTokenGrants, ApiTokens, OidcTrustPolicies, OrgMembers, ServiceAccounts, Users, Workspaces,
};
use entity::{
    api_token_grants, api_tokens, oidc_trust_policies, org_members, service_accounts, users,
};
use oxy_auth::token::StoredKind;
use oxy_auth::token::credential::{blocked_orgs, readable_grants};
use oxy_authz::TokenReach;
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Deserialize;
use uuid::Uuid;

use super::dto::{Here, InventoryTokenDto, inventory_dto};
use crate::server::api::api_keys::activity::{
    ActivityResponse, last_used_at, token_activity, token_activity_in_org,
};
use crate::server::api::user_tokens::dto;
use crate::server::api::user_tokens::error::TokenError;
use crate::server::api::user_tokens::view;

/// `?kind=&owner=&workspace_id=`. Read as text so a malformed id filters to
/// nothing rather than answering the framework's 400.
#[derive(Debug, Default, Deserialize)]
pub struct InventoryQuery {
    /// `personal | legacy_key | service_account | ci`.
    pub kind: Option<String>,
    /// `Token.owner.id`: a user's id, or a service account's.
    pub owner: Option<String>,
    pub workspace_id: Option<String>,
}

/// The filters, parsed. `Err(())` = a filter no token can match.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Filters {
    kind: Option<String>,
    owner: Option<Uuid>,
    workspace_id: Option<Uuid>,
}

impl InventoryQuery {
    pub(super) fn filters(&self) -> Result<Filters, ()> {
        let set = |raw: &Option<String>| raw.clone().filter(|s| !s.trim().is_empty());
        let id = |raw: Option<String>| raw.map(|s| Uuid::parse_str(s.trim()).map_err(|_| ()));
        Ok(Filters {
            kind: set(&self.kind),
            owner: id(set(&self.owner)).transpose()?,
            workspace_id: id(set(&self.workspace_id)).transpose()?,
        })
    }
}

/// The org's side of the question "does this token reach us?".
struct OrgSide {
    org_id: Uuid,
    members: HashSet<Uuid>,
    accounts: HashMap<Uuid, service_accounts::Model>,
    /// Every grant row in this org, revoked ones included.
    grants: Vec<api_token_grants::Model>,
}

impl OrgSide {
    async fn load(db: &DatabaseConnection, org_id: Uuid) -> Result<Self, TokenError> {
        let members = OrgMembers::find()
            .filter(org_members::Column::OrgId.eq(org_id))
            .all(db)
            .await?
            .into_iter()
            .map(|m| m.user_id)
            .collect();
        let accounts = ServiceAccounts::find()
            .filter(service_accounts::Column::OrgId.eq(org_id))
            .all(db)
            .await?
            .into_iter()
            .map(|a| (a.user_id, a))
            .collect();
        let grants = ApiTokenGrants::find()
            .filter(api_token_grants::Column::OrgId.eq(org_id))
            .all(db)
            .await?;
        Ok(Self {
            org_id,
            members,
            accounts,
            grants,
        })
    }

    fn grants_of(&self, row: &api_tokens::Model) -> Vec<api_token_grants::Model> {
        self.grants
            .iter()
            .filter(|g| g.token_id == row.id)
            .cloned()
            .collect()
    }

    fn is_account_token(&self, row: &api_tokens::Model) -> bool {
        dto::acts_as_account(row)
    }

    /// See the module docs for the three ways in.
    fn lists(&self, row: &api_tokens::Model) -> bool {
        if self.is_account_token(row) {
            return self.accounts.contains_key(&row.principal_user_id);
        }
        let member = self.members.contains(&row.principal_user_id);
        if dto::is_legacy(row) || row.all_access {
            return member;
        }
        let here = self.grants_of(row);
        let ended = !here.is_empty() && here.iter().all(|g| g.revoked_at.is_some());
        let carried = row.platform || row.partner;
        !here.is_empty() && (member || carried || ended)
    }

    /// Whether the token reaches `workspace_id`, a workspace of this org.
    fn reaches_workspace(&self, row: &api_tokens::Model, workspace_id: Uuid) -> bool {
        if dto::is_legacy(row) {
            return true;
        }
        let here = self.grants_of(row);
        let reach = TokenReach {
            all_access: row.all_access && !self.is_account_token(row),
            platform: row.platform,
            partner: row.partner,
            grants: readable_grants(&here).unwrap_or_default(),
            blocked_orgs: blocked_orgs(&here),
        };
        reach.workspace_ceiling(self.org_id, workspace_id).is_some()
    }

    /// The live tokens that might be listed: the org's accounts', its members'
    /// all-access ones, and any with a grant row here. `only` narrows to one.
    async fn candidates(
        &self,
        db: &DatabaseConnection,
        only: Option<Uuid>,
    ) -> Result<Vec<api_tokens::Model>, TokenError> {
        let granted: HashSet<Uuid> = self.grants.iter().map(|g| g.token_id).collect();
        let people = [
            StoredKind::Personal.as_str(),
            StoredKind::LegacyKey.as_str(),
        ];
        let of_members = Condition::all()
            .add(api_tokens::Column::Kind.is_in(people))
            .add(api_tokens::Column::AllAccess.eq(true))
            .add(api_tokens::Column::PrincipalUserId.is_in(self.members.iter().copied()));
        let of_accounts = Condition::all()
            .add(
                api_tokens::Column::Kind
                    .is_in([StoredKind::ServiceAccount.as_str(), StoredKind::Ci.as_str()]),
            )
            .add(api_tokens::Column::PrincipalUserId.is_in(self.accounts.keys().copied()));
        let mut query = ApiTokens::find()
            .filter(api_tokens::Column::RevokedAt.is_null())
            .filter(
                Condition::any()
                    .add(of_members)
                    .add(of_accounts)
                    .add(api_tokens::Column::Id.is_in(granted)),
            );
        if let Some(id) = only {
            query = query.filter(api_tokens::Column::Id.eq(id));
        }
        Ok(query.all(db).await?)
    }

    /// The tokens the inventory lists, newest first.
    async fn listed(
        &self,
        db: &DatabaseConnection,
        only: Option<Uuid>,
    ) -> Result<Vec<api_tokens::Model>, TokenError> {
        let rows = self.candidates(db, only).await?;
        let inactive = view::inactive_keys(db, &rows).await?;
        let mut rows: Vec<api_tokens::Model> = rows
            .into_iter()
            .filter(|row| self.lists(row))
            .filter(|row| !inactive.contains(&dto::legacy_key_id(row)))
            .collect();
        rows.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
        Ok(rows)
    }

    /// Each row's owner label: the account's name, or the person's address
    /// (their name, for one with none).
    async fn owner_labels(
        &self,
        db: &DatabaseConnection,
        rows: &[api_tokens::Model],
    ) -> Result<HashMap<Uuid, String>, TokenError> {
        let mut labels: HashMap<Uuid, String> = self
            .accounts
            .values()
            .map(|a| (a.user_id, a.name.clone()))
            .collect();
        let people: HashSet<Uuid> = rows
            .iter()
            .map(|r| r.principal_user_id)
            .filter(|id| !labels.contains_key(id))
            .collect();
        if !people.is_empty() {
            let found = Users::find()
                .filter(users::Column::Id.is_in(people))
                .all(db)
                .await?;
            labels.extend(found.into_iter().map(|u| (u.id, u.label().to_string())));
        }
        Ok(labels)
    }
}

/// Whether the org's CI already uses trusted access: a live trust policy, on
/// an account that is not disabled. Feeds the inventory's
/// `long_lived_while_trusted_access` flag.
async fn uses_trusted_access(db: &DatabaseConnection, org_id: Uuid) -> Result<bool, TokenError> {
    let accounts: Vec<Uuid> = OidcTrustPolicies::find()
        .filter(oidc_trust_policies::Column::OrgId.eq(org_id))
        .filter(oidc_trust_policies::Column::DisabledAt.is_null())
        .all(db)
        .await?
        .into_iter()
        .map(|p| p.service_account_id)
        .collect();
    if accounts.is_empty() {
        return Ok(false);
    }
    let live = ServiceAccounts::find()
        .filter(service_accounts::Column::UserId.is_in(accounts))
        .filter(service_accounts::Column::OrgId.eq(org_id))
        .filter(service_accounts::Column::DisabledAt.is_null())
        .one(db)
        .await?;
    Ok(live.is_some())
}

/// Whether `workspace_id` is one of the org's. A filter on any other workspace
/// matches nothing — an all-access token would otherwise "reach" a made-up id.
async fn workspace_in_org(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
) -> Result<bool, TokenError> {
    Ok(Workspaces::find_by_id(workspace_id)
        .one(db)
        .await?
        .is_some_and(|w| w.org_id == Some(org_id)))
}

pub(super) async fn list(
    db: &DatabaseConnection,
    org_id: Uuid,
    query: &InventoryQuery,
) -> Result<Vec<InventoryTokenDto>, TokenError> {
    let Ok(filters) = query.filters() else {
        return Ok(Vec::new());
    };
    if let Some(workspace_id) = filters.workspace_id
        && !workspace_in_org(db, org_id, workspace_id).await?
    {
        return Ok(Vec::new());
    }
    let org = OrgSide::load(db, org_id).await?;
    let rows: Vec<api_tokens::Model> = org
        .listed(db, None)
        .await?
        .into_iter()
        .filter(|row| {
            filters
                .kind
                .as_ref()
                .is_none_or(|k| *k == dto::wire_kind(row))
        })
        .filter(|row| filters.owner.is_none_or(|o| o == row.principal_user_id))
        .filter(|row| {
            filters
                .workspace_id
                .is_none_or(|w| org.reaches_workspace(row, w))
        })
        .collect();
    let owners = org.owner_labels(db, &rows).await?;
    let tokens = view::tokens(db, &rows, &owners).await?;
    let here = Here {
        org_id,
        trusted_access: uses_trusted_access(db, org_id).await?,
        now: Utc::now(),
    };
    Ok(tokens
        .into_iter()
        .map(|t| inventory_dto(t, &here))
        .collect())
}

/// One token the inventory lists, or 404 — a token that does not reach the
/// org reads the same as one that does not exist.
pub(super) async fn find(
    db: &DatabaseConnection,
    org_id: Uuid,
    token_id: Uuid,
) -> Result<api_tokens::Model, TokenError> {
    let org = OrgSide::load(db, org_id).await?;
    org.listed(db, Some(token_id))
        .await?
        .pop()
        .ok_or(TokenError::NotFound)
}

/// A listed token's activity **in this org**. The org's own (service-account)
/// token has all of it; a person's has only this org's events, and no usage
/// history or request details — those may describe other orgs.
pub(super) async fn activity(
    db: &DatabaseConnection,
    org_id: Uuid,
    token_id: Uuid,
    limit: u64,
) -> Result<ActivityResponse, TokenError> {
    let row = find(db, org_id, token_id).await?;
    let fallback = row.last_used_at.map(|at| last_used_at(at.into()));
    if dto::acts_as_account(&row) {
        return Ok(token_activity(db, row.id, fallback, limit).await?);
    }
    Ok(token_activity_in_org(db, row.id, org_id, fallback, limit).await?)
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
