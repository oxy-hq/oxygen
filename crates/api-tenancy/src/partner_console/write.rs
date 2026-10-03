//! Partner member mutations for `/api/partners/{id}/orgs/{org_id}/members`.
//!
//! Every handler passes through [`require_org_scope`] — the cross-tenant
//! boundary, where the `PartnerCap` ring decides both capability and ownership
//! against the partner's real managed-org subtree. On top of that, a hard
//! **owner guardrail**: a partner may only manage `Member`/`Admin` roles. It can
//! never invite with, promote to, demote, or remove an **Owner** — that would
//! let a partner seize the tenant it merely manages. This also preserves the
//! org's last-owner invariant by construction (partners never touch owners).

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use chrono::Utc;
use entity::org_invitations::{self, InviteStatus};
use entity::org_members::{self, OrgRole};
use entity::prelude::{OrgMembers, Organizations, Users};
use entity::users;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use uuid::Uuid;

use super::{db, internal, require_org_scope};
use crate::partner_console::partner_context::PartnerActor;
use oxy_app::server::api::organizations::{
    find_live_invitation, normalize_invite_email, supersede_expired_invitations,
};
use oxy_app_core::audit::{self, ActorType, AuditEntry};
use oxy_server_authz::partner_authz::PartnerCapability;

/// Parse a target role and reject `Owner` — the partner guardrail. Returns the
/// role or `403`.
fn partner_assignable_role(raw: &str) -> Result<OrgRole, StatusCode> {
    let role = OrgRole::from_str(raw).map_err(|_| StatusCode::BAD_REQUEST)?;
    if !oxy_app_core::member_authz::partner_may_assign(&role) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(role)
}

/// Normalize + validate an invitee address.
///
/// Delegates to the org-side normalizer rather than keeping a local
/// `contains('@')` check: both paths write to the same table and now share the
/// same dedupe helpers, so a looser validator here would mint invitations for
/// addresses the org paths reject — and the stored form has to match, since the
/// dedupe compares emails by equality on the normalized value.
fn normalize_email(raw: &str) -> Result<String, StatusCode> {
    normalize_invite_email(raw)
}

#[derive(Deserialize)]
pub struct InviteMemberBody {
    pub email: String,
    pub role: String,
}

#[derive(Serialize)]
pub struct InvitationResponse {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub token: String,
}

/// `POST /partners/{id}/orgs/{org_id}/members` — invite a member to a managed
/// org (pending invitation; the invitee accepts via the normal magic-link flow).
pub async fn invite_member(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_id, org_id)): Path<(Uuid, Uuid)>,
    headers: axum::http::HeaderMap,
    Json(body): Json<InviteMemberBody>,
) -> Result<Json<InvitationResponse>, StatusCode> {
    let db = db().await?;
    require_org_scope(&db, &scope, org_id, PartnerCapability::ManageMembers).await?;

    let role = partner_assignable_role(&body.role)?;
    let email = normalize_email(&body.email)?;

    // Reject if the email already belongs to a member of this org.
    if let Some(user) = Users::find()
        .filter(users::Column::Email.eq(&email))
        .one(&db)
        .await
        .map_err(internal("lookup invited user"))?
        && OrgMembers::find()
            .filter(org_members::Column::OrgId.eq(org_id))
            .filter(org_members::Column::UserId.eq(user.id))
            .one(&db)
            .await
            .map_err(internal("membership check"))?
            .is_some()
    {
        return Err(StatusCode::CONFLICT);
    }

    let now = Utc::now().fixed_offset();

    // Reject only a *live* duplicate. A lapsed invitation is superseded below,
    // never a permanent block — same rule as the org-side invite paths, via the
    // same helpers so this third call site can't drift from them.
    if find_live_invitation(&db, org_id, &email, now)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_some()
    {
        return Err(StatusCode::CONFLICT);
    }

    let token = Uuid::new_v4().to_string();

    let txn = db.begin().await.map_err(internal("begin invite txn"))?;
    supersede_expired_invitations(&txn, org_id, &email, now)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let invitation = org_invitations::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org_id),
        email: ActiveValue::Set(email.clone()),
        role: ActiveValue::Set(role.clone()),
        invited_by: ActiveValue::Set(actor.id),
        token: ActiveValue::Set(token.clone()),
        status: ActiveValue::Set(InviteStatus::Pending),
        expires_at: ActiveValue::Set((Utc::now() + chrono::Duration::days(7)).fixed_offset()),
        created_at: ActiveValue::Set(now),
    }
    .insert(&txn)
    .await
    .map_err(internal("insert invitation"))?;
    txn.commit().await.map_err(internal("commit invite txn"))?;

    audit::record_best_effort(
        &db,
        AuditEntry::new(actor.label().to_string(), "partner.member.invited")
            .actor(actor.id, ActorType::PartnerAdmin)
            .partner(scope.partner_id)
            .org(org_id)
            .target("org_invitation", email.clone(), email)
            .metadata(serde_json::json!({ "role": role.as_str() })),
    )
    .await;

    // Dispatch the invite email on the same path the org-admin invite uses, so a
    // partner-initiated invite actually reaches the invitee (review #3). Fire and
    // forget: a mail failure must not roll back the invitation row (the token is
    // still the source of truth and can be shared manually).
    let org_name = Organizations::find_by_id(org_id)
        .one(&db)
        .await
        .map_err(internal("load org"))?
        .map(|o| o.name)
        .unwrap_or_else(|| "your organization".to_string());
    let base_url = oxy_app::surface::session::extract_base_url_from_headers(&headers);
    let (to_email, token_clone) = (invitation.email.clone(), token.clone());
    let (inviter_name, inviter_email) = (actor.name.clone(), actor.email.clone());
    tokio::spawn(async move {
        if let Err(e) = oxy_app::server::api::organizations::send_invitation_email(
            &to_email,
            &token_clone,
            &base_url,
            &inviter_name,
            inviter_email.as_deref().unwrap_or(""),
            &org_name,
        )
        .await
        {
            tracing::error!("partner invite email failed for {to_email}: {e}");
        }
    });

    Ok(Json(InvitationResponse {
        id: invitation.id,
        email: invitation.email,
        role: role.as_str().to_string(),
        token,
    }))
}

#[derive(Deserialize)]
pub struct UpdateRoleBody {
    pub role: String,
}

/// `PATCH .../members/{user_id}` — change a member's role. Never touches Owners
/// and never grants Owner.
pub async fn update_member_role(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_id, org_id, user_id)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<UpdateRoleBody>,
) -> Result<StatusCode, StatusCode> {
    let db = db().await?;
    require_org_scope(&db, &scope, org_id, PartnerCapability::ManageMembers).await?;
    let new_role = partner_assignable_role(&body.role)?;

    let target = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(&db)
        .await
        .map_err(internal("load member"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Guardrail: a partner cannot modify an Owner.
    if !oxy_app_core::member_authz::partner_may_modify(&target.role) {
        return Err(StatusCode::FORBIDDEN);
    }
    let before_role = target.role.clone();

    // Same-transaction audit, matching the org-admin role-change path: a member
    // role change is a privileged mutation, so if its audit write fails the role
    // change rolls back rather than standing unaudited.
    let txn = db
        .begin()
        .await
        .map_err(internal("begin role-change txn"))?;
    let mut model: org_members::ActiveModel = target.into();
    model.role = ActiveValue::Set(new_role.clone());
    model.updated_at = ActiveValue::Set(Utc::now().fixed_offset());
    model.update(&txn).await.map_err(internal("update role"))?;
    audit::record_in_txn(
        &txn,
        AuditEntry::new(actor.label().to_string(), "partner.member.role_updated")
            .actor(actor.id, ActorType::PartnerAdmin)
            .partner(scope.partner_id)
            .org(org_id)
            .target("org_member", user_id.to_string(), String::new())
            .change(
                serde_json::json!({ "role": before_role.as_str() }),
                serde_json::json!({ "role": new_role.as_str() }),
            ),
    )
    .await
    .map_err(internal("audit role change"))?;
    txn.commit()
        .await
        .map_err(internal("commit role-change txn"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE .../members/{user_id}` — remove a member. Never removes an Owner
/// (which also preserves the org's last-owner invariant).
pub async fn remove_member(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_id, org_id, user_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = db().await?;
    require_org_scope(&db, &scope, org_id, PartnerCapability::ManageMembers).await?;

    let target = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(&db)
        .await
        .map_err(internal("load member"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    if !oxy_app_core::member_authz::partner_may_modify(&target.role) {
        return Err(StatusCode::FORBIDDEN);
    }

    // Same-transaction audit, matching the org-admin removal path: if the audit
    // write fails, the removal rolls back rather than standing unaudited.
    let txn = db.begin().await.map_err(internal("begin remove txn"))?;
    OrgMembers::delete_by_id(target.id)
        .exec(&txn)
        .await
        .map_err(internal("remove member"))?;
    audit::record_in_txn(
        &txn,
        AuditEntry::new(actor.label().to_string(), "partner.member.removed")
            .actor(actor.id, ActorType::PartnerAdmin)
            .partner(scope.partner_id)
            .org(org_id)
            .target("org_member", user_id.to_string(), String::new()),
    )
    .await
    .map_err(internal("audit member removal"))?;
    txn.commit().await.map_err(internal("commit remove txn"))?;
    Ok(StatusCode::NO_CONTENT)
}
