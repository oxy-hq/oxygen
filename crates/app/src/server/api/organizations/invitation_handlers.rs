use std::collections::HashMap;
use std::str::FromStr;

use axum::extract::{Json, Path};
use axum::http::{HeaderMap, StatusCode};
use chrono::Utc;
use entity::org_invitations;
use entity::org_invitations::InviteStatus;
use entity::org_members;
use entity::org_members::OrgRole;
use entity::organizations;
use entity::prelude::*;
use oxy::database::client::establish_connection;
use oxy::database::filters::UserQueryFilterExt;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QueryOrder,
    TransactionTrait,
};
use uuid::Uuid;

use crate::server::api::middlewares::role_guards::OrgAdmin;
use oxy_app_core::audit;
use oxy_app_core::member_authz;

use super::dto::*;
use super::ops::*;

// Invitations

/// POST /orgs/:org_id/invitations
pub async fn create_invitation(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    headers: HeaderMap,
    Json(req): Json<InviteRequest>,
) -> Result<Json<InvitationResponse>, StatusCode> {
    // Validate role.
    let role = OrgRole::from_str(&req.role).map_err(|_| StatusCode::BAD_REQUEST)?;
    let invited_role = role.clone();

    // Only Owners can invite with the Owner role — the rule lives in member_authz, so
    // this site can't drift from the role-change path that shares it.
    member_authz::authorize_owner_grant(&ctx.membership.role, role == OrgRole::Owner)?;

    // Normalize + format-check the invited email. Lowercased for
    // case-insensitive comparisons against existing rows.
    let invited_email = normalize_invite_email(&req.email)?;

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Reject if the email already belongs to a member of this org.
    if let Some(existing_user) = Users::find()
        .filter_by_email(&invited_email)
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to lookup invited user: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
    {
        let already_member = OrgMembers::find()
            .filter(org_members::Column::OrgId.eq(ctx.org.id))
            .filter(org_members::Column::UserId.eq(existing_user.id))
            .one(&db)
            .await
            .map_err(|e| {
                tracing::error!("Failed to check existing membership: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
        if already_member.is_some() {
            return Err(StatusCode::CONFLICT);
        }
    }

    let now = Utc::now().fixed_offset();

    // Only a *live* invitation blocks a new one. An expired one is superseded
    // below instead of rejecting forever — see `org_invitations::live_pending`.
    //
    // Advisory, not a guarantee: under READ COMMITTED two concurrent invites to
    // the same address can both pass this check and both insert. Running it
    // inside the transaction below would not change that — only a partial
    // unique index on `(org_id, lower(email)) WHERE status='pending'` would.
    // The race is benign (two live tokens; whichever is accepted first wins and
    // the other lapses), so it is left open rather than papered over here.
    if find_live_invitation(&db, ctx.org.id, &invited_email, now)
        .await?
        .is_some()
    {
        return Err(StatusCode::CONFLICT);
    }

    let token = Uuid::new_v4().to_string();
    let expires_at = (Utc::now() + chrono::Duration::days(7)).fixed_offset();

    // Supersede + insert atomically: a crash between the two would otherwise
    // drop the old invite without minting its replacement.
    let txn = db.begin().await.map_err(|e| {
        tracing::error!("Failed to begin transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let superseded = supersede_expired_invitations(&txn, ctx.org.id, &invited_email, now).await?;

    let invitation = org_invitations::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(ctx.org.id),
        email: ActiveValue::Set(invited_email.clone()),
        role: ActiveValue::Set(role),
        invited_by: ActiveValue::Set(ctx.membership.user_id),
        token: ActiveValue::Set(token),
        status: ActiveValue::Set(InviteStatus::Pending),
        expires_at: ActiveValue::Set(expires_at),
        created_at: ActiveValue::Set(now),
    };
    let invitation = invitation.insert(&txn).await.map_err(|e| {
        tracing::error!("Failed to insert invitation: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // An invitation is a *pending* authority grant — the row is what a later
    // "how did this person get Admin?" investigation traces back to. Best-effort,
    // not in-txn: the invite is already committed and useful, and an audit failure
    // must not make the operator retry a send. (The role change it eventually
    // produces IS recorded durably, in `accept_invitation`'s membership write.)
    audit::record_best_effort(
        &db,
        audit::AuditEntry::new(actor.label().to_string(), "org.invitation.created")
            .actor(actor.id, audit::ActorType::User)
            .org(ctx.org.id)
            .target(
                "invitation",
                invitation.id.to_string(),
                invitation.email.clone(),
            )
            .change(
                serde_json::json!(null),
                serde_json::json!({ "role": invited_role.as_str() }),
            )
            .metadata(serde_json::json!({
                "via_global_override": ctx.is_global_override,
                // Non-zero means this invite replaced a lapsed one, which is
                // also why the recipient's older link stopped resolving.
                "superseded_expired": superseded,
            })),
    )
    .await;

    // Fire off the invitation email in the background. Failure to send does not
    // block the response — the DB row + returned token remain the source of truth.
    let inviter = Users::find_by_id(ctx.membership.user_id)
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to lookup inviter: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let base_url = crate::server::api::auth::extract_link_base_for_authenticated_request(&headers);
    let inviter_name = inviter
        .as_ref()
        .map(|u| u.name.clone())
        .unwrap_or_else(|| "A teammate".to_string());
    // The reply-to shown in the invitation email. `and_then`, not `map`: the
    // inviter's address is itself optional now, and an inviter without one
    // simply has no reply-to — which the template already handles. The
    // admin-invite path in `orgs_admin` resolves the same field the same way.
    let inviter_email = inviter.as_ref().and_then(|u| u.email.clone());
    let to_email = invitation.email.clone();
    let token_clone = invitation.token.clone();
    let org_name = ctx.org.name.clone();
    tokio::spawn(async move {
        if let Err(e) = send_invitation_email(
            &to_email,
            &token_clone,
            &base_url,
            &inviter_name,
            inviter_email.as_deref().unwrap_or(""),
            &org_name,
        )
        .await
        {
            tracing::error!("Failed to send invitation email: {e}");
        }
    });

    Ok(Json(InvitationResponse {
        id: invitation.id,
        email: invitation.email,
        role: invitation.role.as_str().to_string(),
        token: invitation.token,
        status: invitation.status.as_str().to_string(),
        expires_at: invitation.expires_at.to_rfc3339(),
        created_at: invitation.created_at.to_rfc3339(),
    }))
}

/// POST /orgs/:org_id/invitations/bulk
///
/// All-or-nothing: every invitation is validated and inserted in a single
/// transaction. If any one fails (duplicate email, already a member, role
/// violation), the whole batch is rolled back so the caller never has to
/// reason about partial state. Emails are spawned only after a successful
/// commit.
pub async fn create_bulk_invitations(
    OrgAdmin(ctx): OrgAdmin,
    headers: HeaderMap,
    Json(req): Json<BulkInviteRequest>,
) -> Result<Json<BulkInviteResponse>, StatusCode> {
    if req.invitations.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Guard against pathological payloads (e.g. a misconfigured CSV import)
    // landing a single request big enough to block the connection while we
    // validate + write thousands of rows in one transaction.
    const MAX_BULK_INVITES: usize = 50;
    if req.invitations.len() > MAX_BULK_INVITES {
        return Err(StatusCode::UNPROCESSABLE_ENTITY);
    }

    // Pre-validate every entry (role parse, owner-grant restriction, non-empty
    // email) and normalize emails. Reject the whole request on any input
    // error before touching the DB. Also catches dupes inside the same batch.
    let mut prepared: Vec<(String, OrgRole)> = Vec::with_capacity(req.invitations.len());
    let mut seen_emails: std::collections::HashSet<String> = std::collections::HashSet::new();
    for inv in &req.invitations {
        let role = OrgRole::from_str(&inv.role).map_err(|_| StatusCode::BAD_REQUEST)?;
        member_authz::authorize_owner_grant(&ctx.membership.role, role == OrgRole::Owner)?;
        let email = normalize_invite_email(&inv.email)?;
        if !seen_emails.insert(email.clone()) {
            return Err(StatusCode::CONFLICT);
        }
        prepared.push((email, role));
    }

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let txn = db.begin().await.map_err(|e| {
        tracing::error!("Failed to begin transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let now = Utc::now().fixed_offset();
    let expires_at = (Utc::now() + chrono::Duration::days(7)).fixed_offset();
    let mut inserted: Vec<org_invitations::Model> = Vec::with_capacity(prepared.len());

    for (email, role) in prepared {
        // Reject if the email already belongs to a member of this org.
        if let Some(existing_user) = Users::find()
            .filter_by_email(&email)
            .one(&txn)
            .await
            .map_err(|e| {
                tracing::error!("Failed to lookup invited user: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
        {
            let already_member = OrgMembers::find()
                .filter(org_members::Column::OrgId.eq(ctx.org.id))
                .filter(org_members::Column::UserId.eq(existing_user.id))
                .one(&txn)
                .await
                .map_err(|e| {
                    tracing::error!("Failed to check existing membership: {e}");
                    StatusCode::INTERNAL_SERVER_ERROR
                })?;
            if already_member.is_some() {
                return Err(StatusCode::CONFLICT);
            }
        }

        // Only a live invitation blocks; a lapsed one is superseded. Same rule
        // as the single-invite path, via the same helpers.
        if find_live_invitation(&txn, ctx.org.id, &email, now)
            .await?
            .is_some()
        {
            return Err(StatusCode::CONFLICT);
        }
        supersede_expired_invitations(&txn, ctx.org.id, &email, now).await?;

        let invitation = org_invitations::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            org_id: ActiveValue::Set(ctx.org.id),
            email: ActiveValue::Set(email),
            role: ActiveValue::Set(role),
            invited_by: ActiveValue::Set(ctx.membership.user_id),
            token: ActiveValue::Set(Uuid::new_v4().to_string()),
            status: ActiveValue::Set(InviteStatus::Pending),
            expires_at: ActiveValue::Set(expires_at),
            created_at: ActiveValue::Set(now),
        };
        let invitation = invitation.insert(&txn).await.map_err(|e| {
            tracing::error!("Failed to insert invitation: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
        inserted.push(invitation);
    }

    // Look up inviter on the same transaction so we have it for email dispatch.
    let inviter = Users::find_by_id(ctx.membership.user_id)
        .one(&txn)
        .await
        .map_err(|e| {
            tracing::error!("Failed to lookup inviter: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let base_url = crate::server::api::auth::extract_link_base_for_authenticated_request(&headers);
    let inviter_name = inviter
        .as_ref()
        .map(|u| u.name.clone())
        .unwrap_or_else(|| "A teammate".to_string());
    // The reply-to shown in the invitation email. `and_then`, not `map`: the
    // inviter's address is itself optional now, and an inviter without one
    // simply has no reply-to — which the template already handles. Matches
    // `orgs_admin`'s admin-invite path.
    let inviter_email = inviter.as_ref().and_then(|u| u.email.clone());
    let org_name = ctx.org.name.clone();
    for invitation in &inserted {
        let to_email = invitation.email.clone();
        let token = invitation.token.clone();
        let base_url = base_url.clone();
        let inviter_name = inviter_name.clone();
        let inviter_email = inviter_email.clone();
        let org_name = org_name.clone();
        tokio::spawn(async move {
            if let Err(e) = send_invitation_email(
                &to_email,
                &token,
                &base_url,
                &inviter_name,
                inviter_email.as_deref().unwrap_or(""),
                &org_name,
            )
            .await
            {
                tracing::error!("Failed to send invitation email: {e}");
            }
        });
    }

    let invitations: Vec<InvitationResponse> = inserted
        .into_iter()
        .map(|inv| InvitationResponse {
            id: inv.id,
            email: inv.email,
            role: inv.role.as_str().to_string(),
            token: inv.token,
            status: inv.status.as_str().to_string(),
            expires_at: inv.expires_at.to_rfc3339(),
            created_at: inv.created_at.to_rfc3339(),
        })
        .collect();

    Ok(Json(BulkInviteResponse { invitations }))
}

/// GET /orgs/:org_id/invitations
pub async fn list_invitations(
    OrgAdmin(ctx): OrgAdmin,
) -> Result<Json<Vec<InvitationSummary>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Every outstanding invitation, expired ones included, flagged so the UI
    // can offer Revoke on a row the admin would otherwise never see. Filtering
    // on `expires_at` here is what made the lockout unrecoverable in-product.
    let now = Utc::now().fixed_offset();
    let invitations = OrgInvitations::find()
        .filter(org_invitations::Column::OrgId.eq(ctx.org.id))
        .filter(org_invitations::Column::Status.eq(InviteStatus::Pending))
        .order_by_desc(org_invitations::Column::CreatedAt)
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query invitations: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let responses: Vec<InvitationSummary> = invitations
        .into_iter()
        .map(|inv| InvitationSummary {
            id: inv.id,
            email: inv.email.clone(),
            role: inv.role.as_str().to_string(),
            token: inv.token.clone(),
            status: inv.status.as_str().to_string(),
            expires_at: inv.expires_at.to_rfc3339(),
            created_at: inv.created_at.to_rfc3339(),
            is_expired: inv.is_expired(now),
        })
        .collect();

    Ok(Json(responses))
}

/// DELETE /orgs/:org_id/invitations/:invitation_id
pub async fn revoke_invitation(
    OrgAdmin(ctx): OrgAdmin,
    Path((_org_id, invitation_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let invitation = OrgInvitations::find_by_id(invitation_id)
        .filter(org_invitations::Column::OrgId.eq(ctx.org.id))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query invitation: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let active: org_invitations::ActiveModel = invitation.into();
    active.delete(&db).await.map_err(|e| {
        tracing::error!("Failed to delete invitation: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /invitations/mine — list non-expired, pending invitations addressed to
/// the authenticated user's email. Powers the post-login "you've been invited"
/// screen shown before the org-creation onboarding step.
pub async fn list_my_invitations(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<Vec<MyInvitationResponse>>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Stored invitation emails are normalized to lowercase at insert time
    // (see create_invitation), so a plain equality match on the lowercased
    // user email covers RFC 5321 §2.4 case-insensitivity without needing
    // a LOWER(...) wrap in SQL.
    // `user.email`, never a display label. A label falls back to the
    // (non-unique) `name`, so a user named after somebody's address would be
    // handed that person's live invitation tokens. A user with no address has
    // no invitations by construction — an invitation is sent to a mailbox.
    let Some(email_lower) = user.email.as_deref().map(str::to_lowercase) else {
        return Ok(Json(Vec::new()));
    };
    let invitations = OrgInvitations::find()
        .filter(org_invitations::Column::Email.eq(email_lower))
        .filter(org_invitations::live_pending(Utc::now().fixed_offset()))
        // Most-recently-sent invites appear first; stable order across refetches
        // avoids the list jittering between renders when users have several.
        .order_by_desc(org_invitations::Column::CreatedAt)
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query pending invitations: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    if invitations.is_empty() {
        return Ok(Json(Vec::new()));
    }

    // Batch-load referenced orgs + inviters in one query each — avoids N+1.
    let org_ids: Vec<Uuid> = invitations.iter().map(|i| i.org_id).collect();
    let inviter_ids: Vec<Uuid> = invitations.iter().map(|i| i.invited_by).collect();

    let orgs = Organizations::find()
        .filter(organizations::Column::Id.is_in(org_ids))
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to load orgs for invitations: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let org_map: HashMap<Uuid, organizations::Model> =
        orgs.into_iter().map(|o| (o.id, o)).collect();

    let inviters = Users::find()
        .filter(entity::users::Column::Id.is_in(inviter_ids))
        .all(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to load inviters: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let inviter_name_map: HashMap<Uuid, String> = inviters
        .into_iter()
        .map(|u| {
            let display = if u.name.is_empty() {
                u.email.unwrap_or_default()
            } else {
                u.name
            };
            (u.id, display)
        })
        .collect();

    let responses: Vec<MyInvitationResponse> = invitations
        .into_iter()
        .filter_map(|inv| {
            let org = org_map.get(&inv.org_id)?;
            Some(MyInvitationResponse {
                id: inv.id,
                token: inv.token,
                role: inv.role.as_str().to_string(),
                expires_at: inv.expires_at.to_rfc3339(),
                created_at: inv.created_at.to_rfc3339(),
                org_id: org.id,
                org_name: org.name.clone(),
                org_slug: org.slug.clone(),
                invited_by_name: inviter_name_map.get(&inv.invited_by).cloned(),
            })
        })
        .collect();

    Ok(Json(responses))
}

/// POST /invitations/:token/accept
pub async fn accept_invitation(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(token): Path<String>,
) -> Result<Json<OrgResponse>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let invitation = OrgInvitations::find()
        .filter(org_invitations::Column::Token.eq(&token))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query invitation: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    // Verify the invitation was sent to this user's email (case-insensitive per RFC 5321).
    // An invitation is addressed to an email, so a user without one can never
    // be its recipient. Note this MUST compare `user.email`, never a display
    // label: a label falls back to `name`, which is not unique, and a user
    // could then accept an invitation addressed to someone else by being named
    // after their address.
    let Some(user_email) = user.email.as_deref() else {
        return Err(StatusCode::FORBIDDEN);
    };
    if invitation.email.to_lowercase() != user_email.to_lowercase() {
        return Err(StatusCode::FORBIDDEN);
    }

    // Check status and expiration — same rule as `live_pending`, applied to a
    // row already fetched by token.
    if !invitation.is_live(Utc::now().fixed_offset()) {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Check if user is already a member.
    let existing_member = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(invitation.org_id))
        .filter(org_members::Column::UserId.eq(user.id))
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to check existing membership: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    if existing_member.is_some() {
        return Err(StatusCode::CONFLICT);
    }

    // Create membership and mark invitation as accepted atomically.
    let txn = db.begin().await.map_err(|e| {
        tracing::error!("Failed to begin transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let now = Utc::now().fixed_offset();
    let member = org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(invitation.org_id),
        user_id: ActiveValue::Set(user.id),
        role: ActiveValue::Set(invitation.role.clone()),
        created_at: ActiveValue::Set(now),
        updated_at: ActiveValue::Set(now),
    };
    member.insert(&txn).await.map_err(|e| {
        tracing::error!("Failed to create membership: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let mut inv_active: org_invitations::ActiveModel = invitation.clone().into();
    inv_active.status = ActiveValue::Set(InviteStatus::Accepted);
    inv_active.update(&txn).await.map_err(|e| {
        tracing::error!("Failed to update invitation status: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    txn.commit().await.map_err(|e| {
        tracing::error!("Failed to commit transaction: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Sync the Stripe seat quantity to reflect the newly added member. Spawned
    // so the user-facing request doesn't block on Stripe; the reconciliation
    // loop is the safety net if this fails.
    if let Ok(svc) = crate::api::billing::billing_service().await {
        let org_id_bg = invitation.org_id;
        tokio::spawn(async move {
            if let Err(e) = svc.sync_seats(org_id_bg).await {
                tracing::warn!(?e, ?org_id_bg, "sync_seats failed after accept_invitation");
            }
        });
    }

    // Return org details.
    let org = Organizations::find_by_id(invitation.org_id)
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query organization: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    Ok(Json(org_response(&org, &invitation.role)))
}
