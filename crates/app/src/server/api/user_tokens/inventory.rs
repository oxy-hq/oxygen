//! `GET /api/{workspace_id}/api-tokens` — the tokens that can reach this
//! workspace, for its admins (API-tokens design §8 Phase 2: "Tokens with
//! access to this workspace"). Read-only: a token is managed by its owner, in
//! Account → Personal access tokens.
//!
//! **Tokens only.** A legacy API key is not a token and is never listed here:
//! it has its own routes (`/api/{workspace_id}/api-keys`) and its own section.
//!
//! A token is listed when it is not revoked, the org has not blocked it, and
//! either:
//!
//! - it is **all-access** and its owner is a member of the workspace's org; or
//! - a live grant covers the workspace — on it, or on its whole org — and its
//!   owner is a member, or the token carries staff or partner standing (which
//!   is how a non-member's grant reaches anything), or it is a
//!   **service-account** token, whose standing is its account's.
//!
//! An org blocks a token by its revoke-grant or by a token policy the token
//! breaks (design §5). Either way the token is inert here, so it is not a
//! token "with access to this workspace"; the org inventory is where a blocked
//! one is shown, with why.
//!
//! Coverage is asked of the same [`TokenReach`] the request path decides with,
//! blocks included, so the list cannot disagree with enforcement.

use std::collections::{HashMap, HashSet};

use axum::extract::Path;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use entity::prelude::{ApiTokenGrants, ApiTokens, OrgMembers, Users, Workspaces};
use entity::{api_token_grants, api_tokens, org_members, users, workspaces};
use oxy::database::client::establish_connection;
use oxy_auth::token::StoredKind;
use oxy_auth::token::credential::{blocked_orgs, readable_grants};
use oxy_authz::{RoleCeiling, TokenReach};
use sea_orm::{ColumnTrait, Condition, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use super::dto::{self, OwnerDto};
use super::error::TokenError;
use super::policy_view;
use crate::server::api::middlewares::role_guards::WorkspaceAdmin;

#[derive(Debug, PartialEq, Serialize)]
pub struct InventoryToken {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub display_prefix: String,
    pub all_access: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub status: &'static str,
    pub owner: OwnerDto,
    /// The highest role the token may act at in this workspace.
    pub role_ceiling_here: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Inventory {
    pub tokens: Vec<InventoryToken>,
}

/// The ceiling `row` holds over the workspace; `None` when it does not reach
/// it. A token with a grant this release cannot read is refused whole at
/// authentication, so it reaches nothing here either.
///
/// `policy_blocked` is whether the org's token policy makes the token inert
/// here ([`policy_view::blocked_in_org`]). The request path adds that org to
/// the credential's blocked orgs beside the ones a revoke-grant left; so does
/// this, and the same reach model answers.
fn ceiling_here(
    row: &api_tokens::Model,
    grants: &[api_token_grants::Model],
    policy_blocked: bool,
    org_id: Uuid,
    workspace_id: Uuid,
) -> Option<RoleCeiling> {
    let own: Vec<api_token_grants::Model> = grants
        .iter()
        .filter(|g| g.token_id == row.id)
        .cloned()
        .collect();
    let mut blocked = blocked_orgs(&own);
    if policy_blocked && !blocked.contains(&org_id) {
        blocked.push(org_id);
    }
    // All-access is "no cap" — unless the org blocked the token, which the
    // reach model reads from `blocked`. A service account is never
    // all-access, whatever its row says.
    let reach = TokenReach {
        all_access: row.all_access && !is_account_token(row),
        platform: row.platform,
        partner: row.partner,
        grants: if row.all_access {
            Vec::new()
        } else {
            readable_grants(&own).ok()?
        },
        blocked_orgs: blocked,
    };
    reach.workspace_ceiling(org_id, workspace_id)
}

fn is_account_token(row: &api_tokens::Model) -> bool {
    dto::acts_as_account(row)
}

/// Whether the token's owner can still act in the org at all: a member, a
/// narrowed token whose standing flags may carry it there, or a service
/// account — which is no member by design, and whose grants are only ever in
/// its own org.
fn owner_reaches(row: &api_tokens::Model, members: &HashSet<Uuid>) -> bool {
    members.contains(&row.principal_user_id)
        || is_account_token(row)
        || (!row.all_access && (row.platform || row.partner))
}

fn inventory_token(
    row: &api_tokens::Model,
    ceiling: RoleCeiling,
    owner_label: String,
    key_inactive: bool,
    now: DateTime<Utc>,
) -> InventoryToken {
    InventoryToken {
        id: row.id,
        name: row.name.clone(),
        kind: dto::wire_kind(row),
        display_prefix: row.display_prefix.clone(),
        all_access: row.all_access,
        expires_at: row.expires_at.map(Into::into),
        last_used_at: row.last_used_at.map(Into::into),
        status: dto::status_of(row, key_inactive, now),
        owner: OwnerDto::of(row, owner_label),
        role_ceiling_here: ceiling.as_str(),
    }
}

/// Tokens only — a legacy API key is never in this list.
fn listed_kinds() -> [&'static str; 3] {
    [
        StoredKind::Personal.as_str(),
        StoredKind::ServiceAccount.as_str(),
        StoredKind::Ci.as_str(),
    ]
}

/// Live, user-owned tokens that might reach the org: the members' all-access
/// ones, and any whose grant names the org.
async fn candidates(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
    members: &HashSet<Uuid>,
) -> Result<(Vec<api_tokens::Model>, Vec<api_token_grants::Model>), TokenError> {
    // Revoked rows too: an org-wide one is the org's block on the token, which
    // `ceiling_here` must see to leave a blocked all-access token out.
    let grants = ApiTokenGrants::find()
        .filter(api_token_grants::Column::OrgId.eq(org_id))
        .filter(
            Condition::any()
                .add(api_token_grants::Column::WorkspaceId.is_null())
                .add(api_token_grants::Column::WorkspaceId.eq(workspace_id)),
        )
        .all(db)
        .await?;
    let granted: HashSet<Uuid> = grants.iter().map(|g| g.token_id).collect();
    let reaching = Condition::any()
        .add(
            Condition::all()
                .add(api_tokens::Column::AllAccess.eq(true))
                .add(api_tokens::Column::PrincipalUserId.is_in(members.iter().copied())),
        )
        .add(api_tokens::Column::Id.is_in(granted));
    let rows = ApiTokens::find()
        .filter(api_tokens::Column::RevokedAt.is_null())
        .filter(api_tokens::Column::Kind.is_in(listed_kinds()))
        .filter(api_tokens::Column::LegacyApiKeyId.is_null())
        .filter(reaching)
        .all(db)
        .await?;
    Ok((rows, grants))
}

async fn owner_labels(
    db: &DatabaseConnection,
    rows: &[api_tokens::Model],
) -> Result<HashMap<Uuid, String>, TokenError> {
    let ids: HashSet<Uuid> = rows.iter().map(|r| r.principal_user_id).collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let users = Users::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await?;
    Ok(users
        .into_iter()
        .map(|u| (u.id, u.email.unwrap_or(u.name)))
        .collect())
}

async fn load(
    db: &DatabaseConnection,
    org_id: Uuid,
    workspace_id: Uuid,
) -> Result<Inventory, TokenError> {
    let members: HashSet<Uuid> = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .all(db)
        .await?
        .into_iter()
        .map(|m| m.user_id)
        .collect();
    let (mut rows, grants) = candidates(db, org_id, workspace_id, &members).await?;
    rows.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(b.id.cmp(&a.id)));
    let labels = owner_labels(db, &rows).await?;
    // Judged by the functions the request path and the org inventory use: a
    // token this org's policy blocks reaches nothing here, so it is not one of
    // the "tokens with access to this workspace".
    let policy_blocked = policy_view::blocked_in_org(db, &rows, &grants, org_id).await?;
    let now = Utc::now();
    let tokens = rows
        .iter()
        .filter(|row| owner_reaches(row, &members))
        .filter_map(|row| {
            let blocked = policy_blocked.contains(&row.id);
            let ceiling = ceiling_here(row, &grants, blocked, org_id, workspace_id)?;
            let label = labels
                .get(&row.principal_user_id)
                .cloned()
                .unwrap_or_default();
            Some(inventory_token(row, ceiling, label, false, now))
        })
        .collect();
    Ok(Inventory { tokens })
}

/// The org of the route's workspace: from the row the workspace middleware
/// attached, else looked up. A workspace with no org has no tokens to list.
async fn org_of(
    db: &DatabaseConnection,
    workspace: Option<Extension<workspaces::Model>>,
    workspace_id: Uuid,
) -> Result<Uuid, TokenError> {
    let org_id = match workspace {
        Some(Extension(row)) => row.org_id,
        None => Workspaces::find_by_id(workspace_id)
            .one(db)
            .await?
            .and_then(|w| w.org_id),
    };
    org_id.ok_or(TokenError::NotFound)
}

/// List the API tokens that can reach this workspace
pub async fn list_workspace_tokens(
    _: WorkspaceAdmin,
    workspace: Option<Extension<workspaces::Model>>,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<Inventory>, TokenError> {
    let db = establish_connection().await?;
    let org_id = org_of(&db, workspace, workspace_id).await?;
    Ok(Json(load(&db, org_id, workspace_id).await?))
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
