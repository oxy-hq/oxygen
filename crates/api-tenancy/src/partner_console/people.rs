//! `/api/partners/{partner_org_id}/people` — the partner **staffing itself**.
//!
//! Two-level governance: Oxy sets the ceiling (what the partner may EVER do); the
//! partner's own org owner/admin decides **who** among their people is a partner
//! **operator**. There is one kind of operator and no per-client scope — access is
//! all-or-nothing, and every operator reaches every client, bounded by the ceiling.
//!
//! Access is **data, not a role**: a row in `partner_role_bindings` hangs off an
//! ordinary `org_members` row. A member of the partner org without one is just an
//! employee using the partner's own Oxy.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::org_members::OrgRole;
use entity::prelude::{OrgMembers, PartnerRoleBindings, Users};
use entity::{org_members, partner_role_bindings, users};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use sea_orm::ActiveModelTrait;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
};
use serde::Serialize;
use std::collections::HashSet;
use uuid::Uuid;

use super::{db, internal};
use crate::partner_console::partner_context::PartnerActor;
use oxy_app_core::audit::{self, ActorType, AuditEntry};

/// Who may change partner access: an **owner or admin of the partner org** (the same
/// people who run any org), or Oxy staff acting as the partner. A plain operator can
/// *use* their access but not hand it out.
///
/// A caller reaches this handler only with a live `PartnerScope`, so if they have no
/// membership in the partner org they must be staff who assumed it (validated by
/// `partner_middleware`) — which is allowed.
///
/// That is exactly the OrgAdmin ring — a real owner/admin, or an operator reaching in
/// via the override — so it is enforced through the unified layer rather than
/// re-deriving `Owner | Admin` here. An inline copy of a ring is how a call site drifts
/// from the model it believes it implements.
async fn require_people_admin(
    db: &DatabaseConnection,
    partner_org_id: Uuid,
    user: &AuthenticatedUser,
) -> Result<(), StatusCode> {
    let membership = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(partner_org_id))
        .filter(org_members::Column::UserId.eq(user.id))
        .one(db)
        .await
        .map_err(internal("load caller membership"))?;
    let legacy = match membership {
        Some(m) => matches!(m.role, OrgRole::Owner | OrgRole::Admin),
        None => true, // no membership + a valid scope ⇒ staff assuming the partner
    };

    let allowed = oxy_server_authz::enforce_for(
        db,
        user.id,
        user.email.as_deref().unwrap_or(""),
        "partner_console.people_admin",
        oxy_server_authz::Action::MemberSetRole,
        oxy_server_authz::Resource::org(partner_org_id),
        legacy,
    )
    .await;
    if allowed {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

#[derive(Serialize)]
pub struct PersonDto {
    /// Their membership in the PARTNER org — the access row's key.
    pub org_member_id: Uuid,
    pub user_id: Uuid,
    pub email: String,
    pub name: Option<String>,
    /// Their role in the partner org itself (owner/admin/member).
    pub org_role: String,
    /// Whether they are a partner operator. `false` = an ordinary employee of the
    /// partner who manages no clients.
    pub has_access: bool,
}

/// `GET /partners/{id}/people` — everyone at the partner, flagged by access.
pub async fn list_people(actor: PartnerActor) -> Result<Json<Vec<PersonDto>>, StatusCode> {
    let db = db().await?;
    let partner_org_id = actor.0.partner_id;

    let members = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(partner_org_id))
        .all(&db)
        .await
        .map_err(internal("load members"))?;
    if members.is_empty() {
        return Ok(Json(Vec::new()));
    }

    let member_ids: Vec<Uuid> = members.iter().map(|m| m.id).collect();
    let with_access: HashSet<Uuid> = PartnerRoleBindings::find()
        .filter(partner_role_bindings::Column::OrgMemberId.is_in(member_ids))
        .all(&db)
        .await
        .map_err(internal("load access rows"))?
        .into_iter()
        .map(|b| b.org_member_id)
        .collect();

    let user_ids: Vec<Uuid> = members.iter().map(|m| m.user_id).collect();
    let people: std::collections::HashMap<Uuid, users::Model> = Users::find()
        .filter(users::Column::Id.is_in(user_ids))
        .all(&db)
        .await
        .map_err(internal("load users"))?
        .into_iter()
        .map(|u| (u.id, u))
        .collect();

    let out = members
        .into_iter()
        .map(|m| {
            let u = people.get(&m.user_id);
            PersonDto {
                org_member_id: m.id,
                user_id: m.user_id,
                email: u.map(|u| u.label().to_string()).unwrap_or_default(),
                name: u.map(|u| u.name.clone()),
                org_role: m.role.as_str().to_string(),
                has_access: with_access.contains(&m.id),
            }
        })
        .collect();
    Ok(Json(out))
}

/// `PUT /partners/{id}/people/{org_member_id}` — grant partner access. Idempotent:
/// granting to someone who already has it is a no-op success.
pub async fn grant_access(
    actor: PartnerActor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((_partner_org_id, org_member_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<PersonDto>, StatusCode> {
    let partner_org_id = actor.0.partner_id;
    let db = db().await?;
    require_people_admin(&db, partner_org_id, &user).await?;

    // The target must be a member of THIS partner org — the same check that stops
    // an admin from granting access to someone else's employee.
    let member = OrgMembers::find_by_id(org_member_id)
        .one(&db)
        .await
        .map_err(internal("load member"))?
        .filter(|m| m.org_id == partner_org_id)
        .ok_or(StatusCode::NOT_FOUND)?;

    let target_email = Users::find_by_id(member.user_id)
        .one(&db)
        .await
        .map_err(internal("load target user"))?
        .map(|u| u.email)
        .unwrap_or_default();

    let already = PartnerRoleBindings::find()
        .filter(partner_role_bindings::Column::OrgMemberId.eq(org_member_id))
        .one(&db)
        .await
        .map_err(internal("load access row"))?;

    if already.is_none() {
        let txn = db.begin().await.map_err(internal("begin grant"))?;
        partner_role_bindings::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_member_id: ActiveValue::Set(org_member_id),
            created_at: ActiveValue::NotSet,
        }
        .insert(&txn)
        .await
        .map_err(internal("insert access row"))?;

        audit::record_in_txn(
            &txn,
            AuditEntry::new(user.label().to_string(), "partner.access.granted")
                .actor(user.id, ActorType::User)
                .partner(partner_org_id)
                .org(partner_org_id)
                .target(
                    "partner_member",
                    org_member_id.to_string(),
                    target_email.clone().unwrap_or_default(),
                ),
        )
        .await
        .map_err(internal("audit access.granted"))?;
        txn.commit().await.map_err(internal("commit grant"))?;
    }

    Ok(Json(PersonDto {
        org_member_id,
        user_id: member.user_id,
        email: target_email.unwrap_or_default(),
        name: None,
        org_role: member.role.as_str().to_string(),
        has_access: true,
    }))
}

/// `DELETE /partners/{id}/people/{org_member_id}` — revoke partner access. The
/// person stays an employee of the partner; they just manage no clients.
pub async fn revoke_access(
    actor: PartnerActor,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((_partner_org_id, org_member_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let partner_org_id = actor.0.partner_id;
    let db = db().await?;
    require_people_admin(&db, partner_org_id, &user).await?;

    // Revoking your OWN access would lock you out of the console you're standing in
    // — the same self-lockout the org model guards against for the last owner.
    let me = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(partner_org_id))
        .filter(org_members::Column::UserId.eq(user.id))
        .one(&db)
        .await
        .map_err(internal("load self"))?;
    if me.map(|m| m.id) == Some(org_member_id) {
        return Err(StatusCode::FORBIDDEN);
    }

    let binding = PartnerRoleBindings::find()
        .filter(partner_role_bindings::Column::OrgMemberId.eq(org_member_id))
        .one(&db)
        .await
        .map_err(internal("load access row"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    let txn = db.begin().await.map_err(internal("begin revoke"))?;
    PartnerRoleBindings::delete_by_id(binding.id)
        .exec(&txn)
        .await
        .map_err(internal("delete access row"))?;

    audit::record_in_txn(
        &txn,
        AuditEntry::new(user.label().to_string(), "partner.access.revoked")
            .actor(user.id, ActorType::User)
            .partner(partner_org_id)
            .org(partner_org_id)
            .target("partner_member", org_member_id.to_string(), String::new()),
    )
    .await
    .map_err(internal("audit access.revoked"))?;
    txn.commit().await.map_err(internal("commit revoke"))?;

    Ok(StatusCode::NO_CONTENT)
}
