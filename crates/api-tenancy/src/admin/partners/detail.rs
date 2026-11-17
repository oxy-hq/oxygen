//! DTOs for the staff partner surface, plus the one loader they all funnel into.
//!
//! Split out of `mod.rs` so the router + handlers stay readable: everything here
//! is shape, not decision.

use axum::http::StatusCode;
use entity::prelude::{
    OrgMembers, Organizations, PartnerCapabilities, PartnerGrants, PartnerOrgs,
    PartnerRoleBindings, Users,
};
use entity::{
    org_members, organizations, partner_capabilities, partner_orgs, partner_role_bindings,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::internal;

/// The ceiling — what Oxy permits this partner AT ALL.
#[derive(Clone, Serialize, Deserialize)]
pub struct CapabilitiesInput {
    pub manage_members: bool,
    /// Publish / unpublish only.
    pub manage_apps: bool,
    /// The data plane. Default OFF.
    #[serde(default)]
    pub develop_apps: bool,
    pub view_audit: bool,
    pub manage_billing: bool,
    pub manage_secrets: bool,
    /// Onboard client orgs. Default OFF — it mints billable tenants.
    #[serde(default)]
    pub create_orgs: bool,
    #[serde(default)]
    pub manage_org_settings: bool,
}

impl CapabilitiesInput {
    /// members / apps / audit / onboarding on; the data plane, billing and
    /// secrets off. `create_orgs` (onboard clients) is on by default because a
    /// reseller channel whose partners can't onboard customers is a support
    /// queue, not a channel — staff can still revoke it per-partner. Billing and
    /// secrets stay owner-only and off (see `require_owner_for_sensitive_caps`).
    pub fn sane_default() -> Self {
        Self {
            manage_members: true,
            manage_apps: true,
            develop_apps: false,
            view_audit: true,
            manage_billing: false,
            manage_secrets: false,
            create_orgs: true,
            manage_org_settings: false,
        }
    }
}

#[derive(Serialize)]
pub struct CapabilitiesDto {
    pub manage_members: bool,
    pub manage_apps: bool,
    pub develop_apps: bool,
    pub view_audit: bool,
    pub manage_billing: bool,
    pub manage_secrets: bool,
    pub create_orgs: bool,
    pub manage_org_settings: bool,
}

impl From<partner_capabilities::Model> for CapabilitiesDto {
    fn from(m: partner_capabilities::Model) -> Self {
        Self {
            manage_members: m.manage_members,
            manage_apps: m.manage_apps,
            develop_apps: m.develop_apps,
            view_audit: m.view_audit,
            manage_billing: m.manage_billing,
            manage_secrets: m.manage_secrets,
            create_orgs: m.create_orgs,
            manage_org_settings: m.manage_org_settings,
        }
    }
}

#[derive(Serialize)]
pub struct PartnerSummary {
    /// The partner IS an org.
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub managed_count: usize,
    pub created_at: String,
}

#[derive(Serialize)]
pub struct ManagedOrgDto {
    pub org_id: Uuid,
    pub org_name: Option<String>,
    pub org_slug: Option<String>,
    pub attached_at: String,
}

/// One member of the partner org. Staff see EVERYONE so they can grant or revoke
/// access to any of them — `has_access` flags who is currently an operator.
#[derive(Serialize)]
pub struct PartnerPersonDto {
    pub org_member_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    /// The person's role in the partner org itself (owner/admin/member).
    pub org_role: String,
    /// Whether they are a partner operator.
    pub has_access: bool,
}

#[derive(Serialize)]
pub struct PartnerDetail {
    pub org_id: Uuid,
    pub name: String,
    pub slug: String,
    pub status: String,
    pub created_at: String,
    /// The CEILING.
    pub capabilities: CapabilitiesDto,
    pub managed_orgs: Vec<ManagedOrgDto>,
    pub people: Vec<PartnerPersonDto>,
}

pub(crate) async fn load_detail(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<PartnerDetail, StatusCode> {
    let grant = PartnerGrants::find_by_id(org_id)
        .one(db)
        .await
        .map_err(internal("load grant"))?
        .ok_or(StatusCode::NOT_FOUND)?;
    let org = Organizations::find_by_id(org_id)
        .one(db)
        .await
        .map_err(internal("load org"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    let capabilities = PartnerCapabilities::find_by_id(org_id)
        .one(db)
        .await
        .map_err(internal("load ceiling"))?
        .map(CapabilitiesDto::from)
        .unwrap_or(CapabilitiesDto {
            manage_members: false,
            manage_apps: false,
            develop_apps: false,
            view_audit: false,
            manage_billing: false,
            manage_secrets: false,
            create_orgs: false,
            manage_org_settings: false,
        });

    // Clients.
    let links = PartnerOrgs::find()
        .filter(partner_orgs::Column::PartnerOrgId.eq(org_id))
        .all(db)
        .await
        .map_err(internal("load clients"))?;
    let client_ids: Vec<Uuid> = links.iter().map(|l| l.managed_org_id).collect();
    let client_meta: HashMap<Uuid, organizations::Model> = if client_ids.is_empty() {
        HashMap::new()
    } else {
        Organizations::find()
            .filter(organizations::Column::Id.is_in(client_ids))
            .all(db)
            .await
            .map_err(internal("load client orgs"))?
            .into_iter()
            .map(|o| (o.id, o))
            .collect()
    };
    let managed_orgs = links
        .into_iter()
        .map(|l| {
            let m = client_meta.get(&l.managed_org_id);
            ManagedOrgDto {
                org_id: l.managed_org_id,
                org_name: m.map(|o| o.name.clone()),
                org_slug: m.map(|o| o.slug.clone()),
                attached_at: l.created_at.to_rfc3339(),
            }
        })
        .collect();

    // Everyone in the partner org — staff grant/revoke access to any of them, so we
    // return all members, flagged by whether they currently hold access.
    let members = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .all(db)
        .await
        .map_err(internal("load org members"))?;
    let member_ids: Vec<Uuid> = members.iter().map(|m| m.id).collect();

    let with_access: HashSet<Uuid> = if member_ids.is_empty() {
        HashSet::new()
    } else {
        PartnerRoleBindings::find()
            .filter(partner_role_bindings::Column::OrgMemberId.is_in(member_ids.clone()))
            .all(db)
            .await
            .map_err(internal("load access rows"))?
            .into_iter()
            .map(|b| b.org_member_id)
            .collect()
    };

    let user_ids: Vec<Uuid> = members.iter().map(|m| m.user_id).collect();
    let emails: HashMap<Uuid, String> = if user_ids.is_empty() {
        HashMap::new()
    } else {
        Users::find()
            .filter(entity::users::Column::Id.is_in(user_ids))
            .all(db)
            .await
            .map_err(internal("load users"))?
            .into_iter()
            .map(|u| (u.id, u.label().to_string()))
            .collect()
    };

    let people = members
        .iter()
        .map(|m| PartnerPersonDto {
            org_member_id: m.id,
            user_id: m.user_id,
            email: emails.get(&m.user_id).cloned().unwrap_or_default(),
            org_role: m.role.as_str().to_string(),
            has_access: with_access.contains(&m.id),
        })
        .collect();

    Ok(PartnerDetail {
        org_id: grant.org_id,
        name: org.name,
        slug: org.slug,
        status: grant.status,
        created_at: grant.created_at.to_rfc3339(),
        capabilities,
        managed_orgs,
        people,
    })
}
