//! `GET /api/user/token-options` — what the create dialog may offer: the orgs
//! and workspaces a grant can name, the caller's role in each (a ceiling above
//! it is accepted and simply capped at use time), and whether the two standing
//! checkboxes apply.
//!
//! Orgs are the caller's own (`via: member`) and, for a partner, the clients it
//! manages (`via: partner`). Staff reach is not enumerated: a staff token that
//! names orgs names them by id.

use std::collections::HashMap;

use axum::Json;
use entity::org_members::OrgRole;
use entity::prelude::{Organizations, WorkspaceMembers, Workspaces};
use entity::workspace_members::WorkspaceRole;
use entity::{organizations, workspace_members, workspaces};
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::SessionOnly;
use oxy_auth::token::policy::OrgPolicy;
use oxy_auth::token::policy_store;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use super::ManageTokens;
use super::error::TokenError;
use super::reach;
use crate::server::authz::{self, PrincipalFacts};

const VIA_MEMBER: &str = "member";
const VIA_PARTNER: &str = "partner";

#[derive(Debug, PartialEq, Serialize)]
pub struct WorkspaceOption {
    pub workspace_id: Uuid,
    pub name: String,
    /// The caller's role there, as a ceiling: `viewer | member | admin | owner`.
    pub role: &'static str,
}

/// What the org's token policy asks of a new token (API-tokens design §5), so
/// the dialog can show the cap and warn before an all-access token is minted
/// into an org that will block it.
#[derive(Debug, PartialEq, Serialize)]
pub struct PolicyDto {
    /// The longest lifetime the org allows, in days. `null` = no cap.
    pub max_lifetime_days: Option<i64>,
    pub allow_all_access_tokens: bool,
}

impl PolicyDto {
    /// The org's policy. An org with no restrictive row holds the defaults:
    /// no cap, all-access allowed.
    fn of(policy: Option<&OrgPolicy>) -> Self {
        let policy = policy.copied().unwrap_or_default();
        Self {
            max_lifetime_days: policy.max_lifetime_days.map(i64::from),
            allow_all_access_tokens: policy.allow_all_access_tokens,
        }
    }
}

#[derive(Debug, PartialEq, Serialize)]
pub struct OrgOption {
    pub org_id: Uuid,
    pub org_name: String,
    pub org_slug: String,
    pub role: &'static str,
    pub via: &'static str,
    pub workspaces: Vec<WorkspaceOption>,
    pub policy: PolicyDto,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct TokenOptions {
    pub orgs: Vec<OrgOption>,
    pub can_platform: bool,
    pub can_partner: bool,
}

/// The orgs a grant may name, with the role to show and how they are reached.
/// A partner acting in a client is an admin there, never an owner.
fn org_reach(facts: &PrincipalFacts) -> Vec<(Uuid, OrgRole, &'static str)> {
    let role_in = |org: &Uuid| {
        if facts.owned_orgs.contains(org) {
            OrgRole::Owner
        } else if facts.admin_orgs.contains(org) {
            OrgRole::Admin
        } else {
            OrgRole::Member
        }
    };
    let mut out: Vec<(Uuid, OrgRole, &'static str)> = facts
        .member_orgs
        .iter()
        .map(|org| (*org, role_in(org), VIA_MEMBER))
        .collect();
    if facts.is_partner() {
        for client in facts.partners.iter().flat_map(|p| &p.client_orgs) {
            if !out.iter().any(|(org, ..)| org == client) {
                out.push((*client, OrgRole::Admin, VIA_PARTNER));
            }
        }
    }
    out
}

/// `max(org-derived role, the workspace's own override)` — an override only
/// ever raises, exactly as `resolve_effective_role` reads it.
fn workspace_role(org_role: &OrgRole, elevated: Option<&WorkspaceRole>) -> WorkspaceRole {
    let derived = match org_role {
        OrgRole::Owner => WorkspaceRole::Owner,
        OrgRole::Admin => WorkspaceRole::Admin,
        OrgRole::Member => WorkspaceRole::Member,
    };
    match elevated {
        Some(role) => std::cmp::max(derived, role.clone()),
        None => derived,
    }
}

/// The three columns of an org the dialog shows.
#[derive(Clone, Debug)]
struct OrgRow {
    id: Uuid,
    name: String,
    slug: String,
}

#[derive(Clone, Debug)]
struct WorkspaceRow {
    id: Uuid,
    org_id: Uuid,
    name: String,
}

/// `policies` holds the orgs whose policy restricts something
/// (`policy_store::restrictive`); an org absent from it holds the defaults.
fn build(
    facts: &PrincipalFacts,
    orgs: &[OrgRow],
    workspaces: &[WorkspaceRow],
    elevated: &HashMap<Uuid, WorkspaceRole>,
    policies: &HashMap<Uuid, OrgPolicy>,
) -> TokenOptions {
    let mut out: Vec<OrgOption> = org_reach(facts)
        .into_iter()
        .filter_map(|(org_id, role, via)| {
            let org = orgs.iter().find(|o| o.id == org_id)?;
            let mut in_org: Vec<WorkspaceOption> = workspaces
                .iter()
                .filter(|w| w.org_id == org_id)
                .map(|w| WorkspaceOption {
                    workspace_id: w.id,
                    name: w.name.clone(),
                    role: workspace_role(&role, elevated.get(&w.id)).as_str(),
                })
                .collect();
            in_org.sort_by(|a, b| a.name.cmp(&b.name));
            Some(OrgOption {
                org_id,
                org_name: org.name.clone(),
                org_slug: org.slug.clone(),
                role: role.as_str(),
                via,
                workspaces: in_org,
                policy: PolicyDto::of(policies.get(&org_id)),
            })
        })
        .collect();
    out.sort_by(|a, b| a.org_name.cmp(&b.org_name));
    TokenOptions {
        orgs: out,
        can_platform: facts.is_staff(),
        can_partner: facts.is_partner(),
    }
}

async fn load(db: &DatabaseConnection, actor: &RequestActor) -> Result<TokenOptions, TokenError> {
    let facts = reach::facts(db, &authz::caller_of(actor)).await?;
    let org_ids: Vec<Uuid> = org_reach(&facts).into_iter().map(|(org, ..)| org).collect();
    if org_ids.is_empty() {
        return Ok(build(&facts, &[], &[], &HashMap::new(), &HashMap::new()));
    }
    // The same read the request path judges a token with, so the dialog and
    // enforcement cannot disagree about what an org allows.
    let policies = policy_store::restrictive(db, &org_ids).await?;
    let orgs: Vec<OrgRow> = Organizations::find()
        .filter(organizations::Column::Id.is_in(org_ids.clone()))
        .all(db)
        .await?
        .into_iter()
        .map(|o| OrgRow {
            id: o.id,
            name: o.name,
            slug: o.slug,
        })
        .collect();
    // Never the nil UUID: that is the legacy `--local` workspace id.
    let workspaces: Vec<WorkspaceRow> = Workspaces::find()
        .filter(workspaces::Column::OrgId.is_in(org_ids))
        .all(db)
        .await?
        .into_iter()
        .filter(|w| !w.id.is_nil())
        .filter_map(|w| {
            Some(WorkspaceRow {
                id: w.id,
                org_id: w.org_id?,
                name: w.name,
            })
        })
        .collect();
    let elevated: HashMap<Uuid, WorkspaceRole> = WorkspaceMembers::find()
        .filter(workspace_members::Column::UserId.eq(actor.id))
        .all(db)
        .await?
        .into_iter()
        .map(|m| (m.workspace_id, m.role))
        .collect();
    Ok(build(&facts, &orgs, &workspaces, &elevated, &policies))
}

/// What the create-token dialog may offer the caller
pub async fn get_token_options(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
) -> Result<Json<TokenOptions>, TokenError> {
    let db = establish_connection().await?;
    Ok(Json(load(&db, &actor).await?))
}

#[cfg(test)]
#[path = "options_tests.rs"]
mod tests;
