//! The wire shape of a token (tokens HTTP contract, "Shared DTOs"), mapped from
//! the stored rows with no database. The lookups that feed it are [`super::view`].

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use entity::{api_token_grants, api_tokens};
use oxy_auth::token::StoredKind;
use serde::Serialize;
use uuid::Uuid;

/// `Token.kind` for every row that mirrors `api_keys`, whatever it is stored
/// as. A token the legacy endpoint minted is stored `personal`, and is exactly
/// as immutable as an `oxy_<hex>` key (design §3.5) — so it reads as one, and
/// the UI offers it the same three actions.
pub(crate) const KIND_LEGACY: &str = "legacy_key";

/// Whether the row mirrors an `api_keys` row: a legacy key, or a token the
/// legacy endpoint minted. All-access with both standings, always.
pub(crate) fn is_legacy(row: &api_tokens::Model) -> bool {
    row.kind == StoredKind::LegacyKey.as_str() || row.legacy_api_key_id.is_some()
}

/// The `api_keys` id behind a legacy row. A legacy key's token id *is* its key
/// id; a legacy-endpoint token names it.
/// Whether the row acts as a service account: an account's own `oxy_sat_`
/// token, or an `oxy_ci_` token a trust policy minted for it.
pub(crate) fn acts_as_account(row: &api_tokens::Model) -> bool {
    StoredKind::parse(&row.kind).is_some_and(StoredKind::acts_as_account)
}

pub(crate) fn legacy_key_id(row: &api_tokens::Model) -> Uuid {
    row.legacy_api_key_id.unwrap_or(row.id)
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GrantDto {
    pub id: Uuid,
    pub kind: String,
    pub org_id: Uuid,
    pub org_name: String,
    /// `null` = every workspace in the org, including future ones.
    pub workspace_id: Option<Uuid>,
    pub workspace_name: Option<String>,
    /// `null` for an `app_publish` grant.
    pub role_ceiling: Option<String>,
    pub app_id: Option<Uuid>,
    pub app_name: Option<String>,
    /// Set when the org ended this grant.
    pub revoked_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OwnerDto {
    /// `user`, or `service_account` for a token an org owns through one.
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// The user's id, or the service account's (`ServiceAccount.id`).
    pub id: Uuid,
    pub label: String,
}

impl OwnerDto {
    pub(crate) fn user(id: Uuid, label: impl Into<String>) -> Self {
        Self {
            kind: "user",
            id,
            label: label.into(),
        }
    }

    pub(crate) fn service_account(id: Uuid, label: impl Into<String>) -> Self {
        Self {
            kind: "service_account",
            id,
            label: label.into(),
        }
    }

    /// The owner of `row`: its principal, as a person or as a service account
    /// — which the row's own kind says.
    pub(crate) fn of(row: &api_tokens::Model, label: impl Into<String>) -> Self {
        if acts_as_account(row) {
            Self::service_account(row.principal_user_id, label)
        } else {
            Self::user(row.principal_user_id, label)
        }
    }
}

/// An org whose token policy makes the token inert there (Phase 5). `reason`
/// is `max_lifetime` or `all_access_disallowed`. Never set for a legacy key.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlockedOrgDto {
    pub org_id: Uuid,
    pub org_name: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TokenDto {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    /// The non-secret leading fragment, as stored — no trailing ellipsis.
    pub display_prefix: String,
    pub last_four: String,
    pub all_access: bool,
    pub platform: bool,
    pub partner: bool,
    pub grants: Vec<GrantDto>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub status: &'static str,
    pub source: String,
    pub owner: OwnerDto,
    pub blocked_orgs: Vec<BlockedOrgDto>,
}

/// The display names behind the ids a grant carries.
#[derive(Clone, Debug, Default)]
pub(crate) struct Names {
    pub orgs: HashMap<Uuid, String>,
    pub workspaces: HashMap<Uuid, String>,
    pub apps: HashMap<Uuid, String>,
}

pub(crate) const STATUS_ACTIVE: &str = "active";
pub(crate) const STATUS_EXPIRED: &str = "expired";
pub(crate) const STATUS_REVOKED: &str = "revoked";

/// `revoked` wins over `expired`: a revoked token stays revoked however its
/// expiry reads. `key_inactive` is a legacy row whose `api_keys` row was
/// revoked by a pod one release back, which writes that table alone.
pub(crate) fn status_of(
    row: &api_tokens::Model,
    key_inactive: bool,
    now: DateTime<Utc>,
) -> &'static str {
    if row.revoked_at.is_some() || key_inactive {
        STATUS_REVOKED
    } else if row
        .expires_at
        .is_some_and(|at| DateTime::<Utc>::from(at) <= now)
    {
        STATUS_EXPIRED
    } else {
        STATUS_ACTIVE
    }
}

pub(crate) fn wire_kind(row: &api_tokens::Model) -> String {
    if is_legacy(row) {
        KIND_LEGACY.to_string()
    } else {
        row.kind.clone()
    }
}

pub(crate) fn grant_dto(grant: &api_token_grants::Model, names: &Names) -> GrantDto {
    let name_of = |names: &HashMap<Uuid, String>, id: Option<Uuid>| {
        id.map(|id| names.get(&id).cloned().unwrap_or_default())
    };
    GrantDto {
        id: grant.id,
        kind: grant.kind.clone(),
        org_id: grant.org_id,
        org_name: names.orgs.get(&grant.org_id).cloned().unwrap_or_default(),
        workspace_id: grant.workspace_id,
        workspace_name: name_of(&names.workspaces, grant.workspace_id),
        role_ceiling: grant.role_ceiling.clone(),
        app_id: grant.app_id,
        app_name: name_of(&names.apps, grant.app_id),
        revoked_at: grant.revoked_at.map(Into::into),
    }
}

/// What the mapping needs beside the row itself.
pub(crate) struct TokenView<'a> {
    /// The row's grants, revoked ones included, oldest first.
    pub grants: &'a [api_token_grants::Model],
    pub names: &'a Names,
    pub owner: OwnerDto,
    pub key_inactive: bool,
    pub now: DateTime<Utc>,
}

pub(crate) fn token_dto(row: &api_tokens::Model, view: TokenView<'_>) -> TokenDto {
    // A legacy row has no grants, and an all-access token's grants are not
    // consulted: neither shows any that a stray row might carry — except the
    // ones an org revoked, which are history either way.
    let shown = |g: &&api_token_grants::Model| {
        !is_legacy(row) && (!row.all_access || g.revoked_at.is_some())
    };
    TokenDto {
        id: row.id,
        name: row.name.clone(),
        kind: wire_kind(row),
        display_prefix: row.display_prefix.clone(),
        last_four: row.last_four.clone(),
        all_access: row.all_access,
        platform: row.platform,
        partner: row.partner,
        grants: view
            .grants
            .iter()
            .filter(|g| g.token_id == row.id)
            .filter(shown)
            .map(|g| grant_dto(g, view.names))
            .collect(),
        expires_at: row.expires_at.map(Into::into),
        last_used_at: row.last_used_at.map(Into::into),
        created_at: row.created_at.into(),
        revoked_at: row.revoked_at.map(Into::into),
        status: status_of(row, view.key_inactive, view.now),
        source: row.source.clone(),
        owner: view.owner,
        // Filled by `view::tokens`, which reads the policies.
        blocked_orgs: Vec::new(),
    }
}

#[cfg(test)]
#[path = "dto_tests.rs"]
mod tests;
