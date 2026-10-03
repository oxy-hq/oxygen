//! `/api/admin/users/*` — the cross-tenant user directory.
//!
//! Surfaces every user in the database, regardless of org, so operators can search by
//! email, inspect their org memberships, and run targeted actions (deactivate, change
//! role, remove from org).
//!
//! **Gated by `cap(Action::PlatformUsers)`, not OXY_OWNER** — the doc said owner-only
//! long after that stopped being true, which mattered here more than most: this is the
//! file whose membership writes needed their own scope fence precisely because
//! non-owner, possibly BOUNDED staff reach it. A header claiming they can't is the
//! reason nobody went looking.

use std::collections::HashMap;
use std::str::FromStr;

use axum::extract::{OriginalUri, Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Json, Router};
use chrono::Utc;
use entity::org_invitations::InviteStatus;
use entity::org_members::OrgRole;
use entity::prelude::AppAdmins;
use entity::users::UserStatus;
use entity::{
    app_admin_scope_orgs, app_admins, org_invitations, org_members, organizations, users,
    workspace_members,
};
use oxy::database::client::establish_connection;
use oxy_app_core::pagination::{self, Paged, trim_overfetch};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseBackend, EntityTrait, FromQueryResult,
    PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, Set, Statement, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use serde_json::json;

use crate::server::api::admin::scope;
use crate::server::router::AppState;
use oxy_app_core::audit;
use sea_orm::ExprTrait;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/users", get(list_users))
        .route("/users/{user_id}", get(get_user_detail))
        .route("/users/{user_id}/status", patch(set_user_status))
        .route(
            "/users/{user_id}/org-memberships",
            post(add_to_org).get(list_org_memberships),
        )
        .route(
            "/users/{user_id}/org-memberships/{org_id}",
            patch(update_role).delete(remove_from_org),
        )
        .route(
            "/users/{user_id}/invitations/{invitation_id}",
            delete(revoke_user_invitation),
        )
}

/// A partner this user operates.
///
/// Not email-keyed: a partner's operators are ordinary members of the partner ORG,
/// so this resolves through `org_members` → `partner_role_bindings` (a row = access).
/// There is one operator role, so the reference is just the partner's identity.
#[derive(Serialize, Clone)]
pub struct UserPartnerRef {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize)]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub last_login_at: String,
    /// Holds a platform grant of ANY role. Kept for existing consumers; it cannot
    /// distinguish a Global Admin from an App Operator, which is why the two fields
    /// below exist.
    pub is_app_admin: bool,
    /// The platform role they hold (`global_admin` | `app_operator`), or `None` for a
    /// non-staff user. This is the column the directory actually shows: "is staff" is
    /// no longer a rank, so a boolean cannot answer "what can this person do".
    pub platform_role: Option<String>,
    /// `true` = the grant reaches every org. Meaningless when `platform_role` is None.
    pub platform_scope_all: bool,
    /// How many orgs a bounded grant reaches. 0 when unbounded or non-staff.
    pub platform_scope_org_count: usize,
    pub org_count: i64,
    /// Partners this user administers. Non-empty ⇒ they are a **Partner Admin**,
    /// a delegated cross-org authority that is invisible from `org_count` alone.
    pub partners: Vec<UserPartnerRef>,
    /// Their highest role across their orgs ("owner" | "admin" | "member"), so
    /// the directory shows tenant *hierarchy*, not just a membership count.
    pub top_org_role: Option<String>,
}

#[derive(Serialize)]
pub struct AdminUserDetail {
    pub id: Uuid,
    pub email: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub last_login_at: String,
    pub picture: Option<String>,
    pub email_verified: bool,
    pub is_app_admin: bool,
    pub org_memberships: Vec<UserOrgMembership>,
    pub workspace_memberships: Vec<UserWorkspaceMembership>,
    /// Outstanding invitations addressed to this user's email — access they
    /// have been offered but not taken up. Answers "why can't they get in?"
    /// when the membership list is empty.
    pub invitations: Vec<UserInvitation>,
    /// Partners this user operates (drives the "Operates" section of the pane).
    pub partners: Vec<UserPartnerRef>,
}

#[derive(Serialize)]
pub struct UserOrgMembership {
    pub org_id: Uuid,
    pub org_slug: String,
    pub org_name: String,
    pub role: String,
    pub joined_at: String,
}

#[derive(Serialize)]
pub struct UserWorkspaceMembership {
    pub workspace_id: Uuid,
    pub workspace_name: String,
    pub role: String,
    pub joined_at: String,
}

/// An outstanding invitation, matched to the user by **email** rather than by
/// user id — invitations predate the account and may never get one. Most
/// people stuck behind a lapsed invite have no `users` row at all, so this
/// section shows what a membership list structurally can't.
#[derive(Serialize)]
pub struct UserInvitation {
    pub id: Uuid,
    pub org_id: Uuid,
    pub org_slug: String,
    pub org_name: String,
    pub role: String,
    pub created_at: String,
    pub expires_at: String,
    /// Derived from `expires_at`, never from `status` — nothing transitions a
    /// row to `expired`, so a lapsed invite still reports itself as pending.
    pub is_expired: bool,
}

#[derive(Deserialize)]
pub struct ListUsersQuery {
    pub search: Option<String>,
    pub status: Option<String>,
    /// Narrow to a role. `global_admin` / `app_operator` select one platform grant;
    /// `staff` selects any. Applied BEFORE pagination — see `emails_with_platform_role`.
    pub role: Option<String>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Deserialize)]
pub struct SetStatusBody {
    pub status: String,
}

#[derive(Deserialize)]
pub struct AddToOrgBody {
    pub org_id: Uuid,
    pub role: String,
}

#[derive(Deserialize)]
pub struct UpdateRoleBody {
    pub role: String,
}

pub async fn list_users(
    OriginalUri(uri): OriginalUri,
    Query(q): Query<ListUsersQuery>,
) -> Result<Paged<AdminUserRow>, StatusCode> {
    let db = establish_connection().await.map_err(internal)?;
    let page = q.page.unwrap_or(0);
    // CLAMPED AT BOTH ENDS. `?page_size=0` past a top-only clamp is an infinite
    // pagination loop, not an empty page: the offset stays 0 on every page while
    // `page + 1` keeps advancing, so every request answers `[]` with a link to
    // the next one. See `oxy_app_core::pagination`.
    //
    // `Ord::clamp` is spelled out for the same reason the `min` was: `ExprTrait`
    // (in scope for the query builders below) blanket-implements its own on
    // every `Into<Expr>` type.
    let page_size = Ord::clamp(q.page_size.unwrap_or(50), 1, 200);

    let mut query = users::Entity::find().order_by_desc(users::Column::LastLoginAt);

    if let Some(needle) = q.search.as_ref().filter(|s| !s.trim().is_empty()) {
        let like = format!("%{}%", needle.trim());
        query = query.filter(
            sea_orm::Condition::any()
                .add(users::Column::Email.contains(like.as_str()))
                .add(users::Column::Name.contains(like.as_str())),
        );
    }
    if let Some(status_str) = q.status.as_ref().filter(|s| !s.is_empty()) {
        let status =
            UserStatus::from_str(status_str.as_str()).map_err(|_| StatusCode::BAD_REQUEST)?;
        query = query.filter(users::Column::Status.eq(status));
    }
    if let Some(role) = q.role.as_ref().filter(|s| !s.trim().is_empty()) {
        let role = role.trim();
        let wanted = match role {
            // Any grant, whatever its role — "show me everyone with console access".
            "staff" => None,
            r => Some(r),
        };
        // An unknown role yields an empty allow-list, so the page comes back empty
        // rather than unfiltered. A filter that silently stops filtering is worse than
        // one that shows nothing.
        let emails = emails_with_platform_role(&db, wanted)
            .await
            .map_err(internal)?;
        if emails.is_empty() {
            // Empty and final — but still routed through `page` rather than a
            // bare `Json(vec![])`, so the response carries `rel="first"`. A
            // paginated endpoint that answers with NO `Link` is
            // indistinguishable from one that never paged, and a client would
            // rightly warn that it cannot tell whether it saw everything.
            return Ok(pagination::page(
                Vec::new(),
                false,
                &uri,
                &[("page", page.saturating_add(1).to_string())],
            ));
        }
        // LOWER() on both sides: grant emails are normalised at write time, `users.email`
        // is not, so a mixed-case staff address matched nothing and vanished from the
        // Staff filter — which this change just made the rail's primary navigation.
        query = query.filter(
            sea_orm::sea_query::Expr::expr(sea_orm::sea_query::Func::lower(
                sea_orm::sea_query::Expr::col(users::Column::Email),
            ))
            .is_in(emails),
        );
    }

    // `page_size + 1`: the extra row is how the `Link: rel="next"` below knows
    // there is a next page, without a second COUNT(*) that could disagree with
    // this one. `trim_overfetch` drops it again before anything sees it.
    let mut rows = query
        .offset(page.saturating_mul(page_size))
        .limit(page_size + 1)
        .all(&db)
        .await
        .map_err(internal)?;
    let has_more = trim_overfetch(&mut rows, page_size);

    // Pre-aggregate the per-row lookups: one IN (...) GROUP BY query for
    // org_count, one IN (...) query for the is_app_admin set. Avoids the
    // O(N) round-trips the previous loop hit on every page render.
    let user_ids: Vec<Uuid> = rows.iter().map(|u| u.id).collect();
    // Platform standing is keyed by ADDRESS. A label falls back to `name`,
    // which is not unique, so a user named after a staff address would be
    // listed as staff. Matches the fix already applied in `get_user_detail`.
    let emails_lower: Vec<String> = rows
        .iter()
        .filter_map(|u| u.email.as_deref().map(str::to_ascii_lowercase))
        .collect();
    let org_counts = count_user_org_memberships_in(&db, &user_ids)
        .await
        .map_err(internal)?;
    let grant_map = lookup_platform_grants_in(&db, &emails_lower)
        .await
        .map_err(internal)?;
    // Two more IN (...) lookups — still O(1) per page, no N+1.
    let partner_map = lookup_partner_admins_in(&db, &user_ids)
        .await
        .map_err(internal)?;
    let role_map = lookup_top_org_role_in(&db, &user_ids)
        .await
        .map_err(internal)?;

    let mut out = Vec::with_capacity(rows.len());
    for u in rows {
        // `Option`, not a blank string. A user with no address holds no
        // platform standing, and saying that with `None` beats a `""` needle
        // that happens not to match anything in the map — the same reasoning
        // that made `is_oxy_owner` refuse a blank. Also collapses three
        // lookups of the same key into one.
        let grant = u
            .email
            .as_deref()
            .map(str::to_ascii_lowercase)
            .and_then(|e| grant_map.get(&e));
        out.push(AdminUserRow {
            id: u.id,
            email: u.label().to_string(),
            name: u.name,
            status: u.status.as_str().to_string(),
            created_at: u.created_at.to_rfc3339(),
            last_login_at: u.last_login_at.to_rfc3339(),
            is_app_admin: grant.is_some(),
            platform_role: grant.map(|g| g.role.clone()),
            platform_scope_all: grant.is_none_or(|g| g.scope_all),
            platform_scope_org_count: grant.map(|g| g.scope_org_count).unwrap_or(0),
            org_count: org_counts.get(&u.id).copied().unwrap_or(0),
            partners: partner_map.get(&u.id).cloned().unwrap_or_default(),
            top_org_role: role_map.get(&u.id).cloned(),
        });
    }

    // `page` here is 0-indexed, which is exactly why the caller is handed a URL
    // rather than a number to increment: `admin/explorer.rs` counts from 1
    // under the same parameter name.
    Ok(pagination::page(
        out,
        has_more,
        &uri,
        // Saturating so `?page=<u64::MAX>` cannot panic a debug build; the
        // offset below saturates for the same reason.
        &[("page", page.saturating_add(1).to_string())],
    ))
}

#[derive(FromQueryResult)]
struct UserIdCountRow {
    user_id: Uuid,
    cnt: i64,
}

#[derive(FromQueryResult)]
struct EmailRow {
    email: String,
}

async fn count_user_org_memberships_in(
    db: &sea_orm::DatabaseConnection,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, i64>, sea_orm::DbErr> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = sql_placeholders(user_ids.len());
    let sql = format!(
        "SELECT user_id, COUNT(*)::bigint AS cnt FROM org_members \
         WHERE user_id IN ({placeholders}) GROUP BY user_id"
    );
    let values: Vec<sea_orm::Value> = user_ids.iter().map(|id| (*id).into()).collect();
    let rows = UserIdCountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.user_id, r.cnt)).collect())
}

/// The platform grant behind a row, if any — role and reach, not just "is staff".
#[derive(Clone)]
pub struct PlatformGrantRef {
    pub role: String,
    pub scope_all: bool,
    pub scope_org_count: usize,
}

/// Platform grants for a page of emails, keyed by lowercased email.
///
/// Typed entity query rather than the hand-built `IN (...)` this replaced: the
/// statement now selects `role` and `scope_all` as well, and raw SQL is exactly what
/// survives a schema change without complaining. One query for the page, plus one for
/// the scope rows of whichever grants are bounded — still O(1) per page.
async fn lookup_platform_grants_in(
    db: &sea_orm::DatabaseConnection,
    emails: &[String],
) -> Result<HashMap<String, PlatformGrantRef>, sea_orm::DbErr> {
    if emails.is_empty() {
        return Ok(HashMap::new());
    }
    let grants = AppAdmins::find()
        .filter(app_admins::Column::Email.is_in(emails.to_vec()))
        .all(db)
        .await?;

    // Count scope rows only for the bounded grants — usually none on a page.
    let bounded: Vec<Uuid> = grants
        .iter()
        .filter(|g| !g.scope_all)
        .map(|g| g.id)
        .collect();
    let mut scope_counts: HashMap<Uuid, usize> = HashMap::new();
    if !bounded.is_empty() {
        for row in app_admin_scope_orgs::Entity::find()
            .filter(app_admin_scope_orgs::Column::AppAdminId.is_in(bounded))
            .all(db)
            .await?
        {
            *scope_counts.entry(row.app_admin_id).or_default() += 1;
        }
    }

    Ok(grants
        .into_iter()
        .map(|g| {
            let scope_org_count = scope_counts.get(&g.id).copied().unwrap_or(0);
            (
                g.email.to_ascii_lowercase(),
                PlatformGrantRef {
                    role: g.role,
                    scope_all: g.scope_all,
                    scope_org_count,
                },
            )
        })
        .collect())
}

/// Emails holding a platform grant, for the `?role=` pre-pagination filter.
///
/// Filtering must narrow the query BEFORE `offset`/`limit`, or pages come back
/// short and the count is a lie — so this reads the (small) grant table first and
/// feeds the result back as a `WHERE email IN (...)`, rather than filtering the
/// assembled rows after the fact.
async fn emails_with_platform_role(
    db: &sea_orm::DatabaseConnection,
    role: Option<&str>,
) -> Result<Vec<String>, sea_orm::DbErr> {
    let mut q = AppAdmins::find();
    if let Some(role) = role {
        q = q.filter(app_admins::Column::Role.eq(role));
    }
    Ok(q.all(db)
        .await?
        .into_iter()
        .map(|g| g.email.to_ascii_lowercase())
        .collect())
}

fn sql_placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn get_user_detail(
    Path(user_id): Path<Uuid>,
) -> Result<Json<AdminUserDetail>, StatusCode> {
    let db = establish_connection().await.map_err(internal)?;
    let user = users::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let is_app_admin = app_admins::Entity::find()
        // `user.email`, never a display label: platform standing is keyed by
        // address, and a label falls back to the (non-unique) `name`, which
        // would let a user be reported as staff by being named after one.
        .filter(
            app_admins::Column::Email.eq(user
                .email
                .as_deref()
                .unwrap_or_default()
                .to_ascii_lowercase()),
        )
        .count(&db)
        .await
        .map_err(internal)?
        > 0;

    let org_memberships = load_user_org_memberships(&db, user.id)
        .await
        .map_err(internal)?;
    let workspace_memberships = load_user_workspace_memberships(&db, user.id)
        .await
        .map_err(internal)?;
    let invitations = load_user_invitations(&db, user.email.as_deref().unwrap_or(""))
        .await
        .map_err(internal)?;
    let partners = lookup_partner_admins_in(&db, &[user.id])
        .await
        .map_err(internal)?
        .remove(&user.id)
        .unwrap_or_default();

    Ok(Json(AdminUserDetail {
        id: user.id,
        email: user.label().to_string(),
        name: user.name,
        status: user.status.as_str().to_string(),
        created_at: user.created_at.to_rfc3339(),
        last_login_at: user.last_login_at.to_rfc3339(),
        picture: user.picture,
        email_verified: user.email_verified,
        is_app_admin,
        org_memberships,
        workspace_memberships,
        invitations,
        partners,
    }))
}

pub async fn list_org_memberships(
    Path(user_id): Path<Uuid>,
) -> Result<Json<Vec<UserOrgMembership>>, StatusCode> {
    let db = establish_connection().await.map_err(internal)?;
    let memberships = load_user_org_memberships(&db, user_id)
        .await
        .map_err(internal)?;
    Ok(Json(memberships))
}

pub async fn set_user_status(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(user_id): Path<Uuid>,
    Json(body): Json<SetStatusBody>,
) -> Result<StatusCode, StatusCode> {
    let new_status =
        UserStatus::from_str(body.status.as_str()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = establish_connection().await.map_err(internal)?;
    let user = users::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let mut active: users::ActiveModel = user.into();
    active.status = Set(new_status);
    active.update(&db).await.map_err(internal)?;
    tracing::info!(
        admin_email = %actor.label(),
        target_id = %user_id,
        new_status = %body.status,
        action = "set_user_status",
        "admin tenant action"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /admin/users/{id}/org-memberships` — put someone in an org, at a role.
///
/// **Audited.** This grants a person standing inside a tenant they were not part of,
/// which is among the most consequential writes staff can make — and until now it wrote
/// nothing to `audit_events`, so there was no record of who was added where or by whom.
/// The partner tier has logged the equivalent action since it shipped; the staff path
/// simply never did, and a nicer UI on top would have made an unrecorded privileged
/// write easier to reach.
///
/// The audit row goes in the SAME transaction as the membership, so a change that
/// cannot be recorded does not happen. Best-effort logging would leave exactly the gap
/// this closes, just narrower.
pub async fn add_to_org(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(user_id): Path<Uuid>,
    Json(body): Json<AddToOrgBody>,
) -> Result<StatusCode, StatusCode> {
    let role = OrgRole::from_str(body.role.as_str()).map_err(|_| StatusCode::BAD_REQUEST)?;
    let db = establish_connection().await.map_err(internal)?;

    let target = users::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let org = organizations::Entity::find_by_id(body.org_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // **Scope.** A bounded grant must not reach outside its orgs — without this a
    // global_admin scoped to one tenant could POST {org_id: <any>, role: "owner"} and
    // hold permanent Owner in a tenant its grant never covered, which is a strictly
    // worse escalation than anything the console's read paths could leak.
    //
    // 404, matching every other out-of-scope answer in this change: an operator with no
    // reach into an org must not learn it exists by being told "forbidden".
    scope::deny_out_of_scope(&db, &actor, body.org_id).await?;

    let tx = db.begin().await.map_err(internal)?;

    // Existence check inside the transaction: outside it, two concurrent adds both see
    // "absent" and the second dies on the unique index with a 500 instead of a 409.
    let existing = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(body.org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(&tx)
        .await
        .map_err(internal)?;
    if existing.is_some() {
        return Err(StatusCode::CONFLICT);
    }

    let now = Utc::now().fixed_offset();
    let role_str = role.as_str().to_string();
    let model = org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: Set(body.org_id),
        user_id: Set(user_id),
        role: Set(role),
        created_at: Set(now),
        updated_at: Set(now),
    };
    model.insert(&tx).await.map_err(internal)?;

    audit::record_in_txn(
        &tx,
        audit::AuditEntry::new(actor.label().to_string(), "member.added")
            .actor(actor.id, audit::ActorType::User)
            .org(body.org_id)
            .target("user", user_id.to_string(), target.label().to_string())
            // `before: null` reads as "held nothing here" — the fact that makes this row
            // meaningful, since it distinguishes a grant from a role change.
            .change(serde_json::Value::Null, json!({ "role": role_str }))
            .metadata(json!({ "org_slug": org.slug, "surface": "admin" })),
    )
    .await
    .map_err(internal)?;

    tx.commit().await.map_err(internal)?;
    Ok(StatusCode::CREATED)
}

pub async fn update_role(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((user_id, org_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateRoleBody>,
) -> Result<StatusCode, Response> {
    let role = OrgRole::from_str(body.role.as_str())
        .map_err(|_| StatusCode::BAD_REQUEST.into_response())?;
    let db = establish_connection().await.map_err(internal_resp)?;
    // Same fence as `add_to_org` — changing or revoking standing inside a tenant is as
    // scoped an act as granting it.
    scope::deny_out_of_scope(&db, &actor, org_id)
        .await
        .map_err(IntoResponse::into_response)?;
    let tx = db.begin().await.map_err(internal_resp)?;

    let membership = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(&tx)
        .await
        .map_err(internal_resp)?
        .ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;

    // Resolved for the audit row's human label — an audit entry keyed only by uuid
    // makes the log unreadable at exactly the moment someone needs to read it.
    let target_email = users::Entity::find_by_id(user_id)
        .one(&tx)
        .await
        .map_err(internal_resp)?
        .map(|u| u.email)
        .unwrap_or_default();

    // Last-owner guard: refuse to demote the only Owner of the org. The
    // owner_count read sits inside the same tx as the role update, so we
    // never race against a concurrent demote/remove.
    let target_was_owner = matches!(membership.role, OrgRole::Owner);
    let demoting = target_was_owner && !matches!(role, OrgRole::Owner);
    if demoting {
        let owner_count = org_members::Entity::find()
            .filter(org_members::Column::OrgId.eq(org_id))
            .filter(org_members::Column::Role.eq(OrgRole::Owner))
            .count(&tx)
            .await
            .map_err(internal_resp)?;
        if owner_count <= 1 {
            let slug = lookup_org_slug(&tx, org_id).await.unwrap_or_default();
            return Err(error_body(
                StatusCode::CONFLICT,
                "last_owner",
                Some(format!(
                    "Cannot demote the last remaining owner of org '{slug}'. \
                     Promote another member to Owner first."
                )),
            ));
        }
    }

    let old_role_str = membership.role.as_str().to_string();
    let mut active: org_members::ActiveModel = membership.into();
    let new_role_str = role.as_str().to_string();
    active.role = Set(role);
    active.updated_at = Set(Utc::now().fixed_offset());
    active.update(&tx).await.map_err(internal_resp)?;

    // Audited in the same transaction — see `add_to_org`. A role change that cannot be
    // recorded must not commit; the `tracing::info!` below is operational visibility,
    // not an audit trail (it is unqueryable, unretained and not tamper-evident).
    audit::record_in_txn(
        &tx,
        audit::AuditEntry::new(actor.label().to_string(), "member.role.updated")
            .actor(actor.id, audit::ActorType::User)
            .org(org_id)
            .target(
                "user",
                user_id.to_string(),
                target_email.clone().unwrap_or_default(),
            )
            .change(
                json!({ "role": old_role_str }),
                json!({ "role": new_role_str }),
            )
            .metadata(json!({ "surface": "admin" })),
    )
    .await
    .map_err(internal_resp)?;

    tx.commit().await.map_err(internal_resp)?;
    tracing::info!(
        admin_email = %actor.label(),
        target_id = %user_id,
        org_id = %org_id,
        new_role = %new_role_str,
        action = "update_role",
        "admin tenant action"
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn remove_from_org(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((user_id, org_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, Response> {
    let db = establish_connection().await.map_err(internal_resp)?;
    // Same fence as `add_to_org` — revoking standing inside a tenant is as scoped an act
    // as granting it.
    scope::deny_out_of_scope(&db, &actor, org_id)
        .await
        .map_err(IntoResponse::into_response)?;
    let tx = db.begin().await.map_err(internal_resp)?;

    // Look the target up first so we know if removing them strips the last
    // owner. owner_count + delete inside the same tx keeps the guard
    // consistent with the write.
    let membership = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .one(&tx)
        .await
        .map_err(internal_resp)?
        .ok_or_else(|| StatusCode::NOT_FOUND.into_response())?;

    let target_email = users::Entity::find_by_id(user_id)
        .one(&tx)
        .await
        .map_err(internal_resp)?
        .map(|u| u.email)
        .unwrap_or_default();

    if matches!(membership.role, OrgRole::Owner) {
        let owner_count = org_members::Entity::find()
            .filter(org_members::Column::OrgId.eq(org_id))
            .filter(org_members::Column::Role.eq(OrgRole::Owner))
            .count(&tx)
            .await
            .map_err(internal_resp)?;
        if owner_count <= 1 {
            let slug = lookup_org_slug(&tx, org_id).await.unwrap_or_default();
            return Err(error_body(
                StatusCode::CONFLICT,
                "last_owner",
                Some(format!(
                    "Cannot remove the last remaining owner of org '{slug}'. \
                     Promote another member to Owner first."
                )),
            ));
        }
    }

    let removed_role = membership.role.as_str().to_string();
    let res = org_members::Entity::delete_many()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user_id))
        .exec(&tx)
        .await
        .map_err(internal_resp)?;
    if res.rows_affected == 0 {
        // Race: someone else already removed the row between our SELECT and
        // DELETE. Treat as 404 for the caller.
        return Err(StatusCode::NOT_FOUND.into_response());
    }
    audit::record_in_txn(
        &tx,
        audit::AuditEntry::new(actor.label().to_string(), "member.removed")
            .actor(actor.id, audit::ActorType::User)
            .org(org_id)
            .target(
                "user",
                user_id.to_string(),
                target_email.clone().unwrap_or_default(),
            )
            // `after: null` — they hold nothing here now. Recording the role they LOST
            // is the part that matters: "removed" without it can't answer whether an
            // owner or a viewer was taken out.
            .change(json!({ "role": removed_role }), serde_json::Value::Null)
            .metadata(json!({ "surface": "admin" })),
    )
    .await
    .map_err(internal_resp)?;

    tx.commit().await.map_err(internal_resp)?;
    tracing::info!(
        admin_email = %actor.label(),
        target_id = %user_id,
        org_id = %org_id,
        action = "remove_from_org",
        "admin tenant action"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Outstanding (`pending`) invitations for `email`, expired ones included.
///
/// Expired rows are the point of this list, not noise to filter: they are
/// invisible to the tenant's own settings screen until it fetches them too,
/// and they're what blocks a re-invite.
async fn load_user_invitations(
    db: &sea_orm::DatabaseConnection,
    email: &str,
) -> Result<Vec<UserInvitation>, sea_orm::DbErr> {
    let now = Utc::now().fixed_offset();
    let invitations = org_invitations::Entity::find()
        .filter(org_invitations::Column::Email.eq(email.to_ascii_lowercase()))
        .filter(org_invitations::Column::Status.eq(InviteStatus::Pending))
        .order_by_desc(org_invitations::Column::CreatedAt)
        .all(db)
        .await?;
    if invitations.is_empty() {
        return Ok(Vec::new());
    }

    let org_ids: Vec<Uuid> = invitations.iter().map(|i| i.org_id).collect();
    let orgs = organizations::Entity::find()
        .filter(organizations::Column::Id.is_in(org_ids))
        .all(db)
        .await?;
    let org_map: HashMap<Uuid, organizations::Model> =
        orgs.into_iter().map(|o| (o.id, o)).collect();

    Ok(invitations
        .into_iter()
        .map(|inv| {
            let org = org_map.get(&inv.org_id);
            UserInvitation {
                id: inv.id,
                org_id: inv.org_id,
                org_slug: org.map(|o| o.slug.clone()).unwrap_or_default(),
                org_name: org.map(|o| o.name.clone()).unwrap_or_default(),
                role: inv.role.as_str().to_string(),
                created_at: inv.created_at.to_rfc3339(),
                expires_at: inv.expires_at.to_rfc3339(),
                is_expired: inv.is_expired(now),
            }
        })
        .collect())
}

/// DELETE /admin/users/{user_id}/invitations/{invitation_id}
///
/// Admin-scoped on purpose. Staff do not pass `OrgAdmin` on the tenant's own
/// `/orgs/{id}/invitations/{id}` route without a live assume session, so the
/// operator console needs its own door — the same reason `add_to_org` and
/// `remove_from_org` exist here rather than reusing the org routes.
pub async fn revoke_user_invitation(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((user_id, invitation_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(internal)?;

    let user = users::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let invitation = org_invitations::Entity::find_by_id(invitation_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;

    // **Scope.** An invitation belongs to the org that issued it, and the path names
    // only the invitee — so without this a grant bounded to one tenant could revoke
    // another tenant's pending invitation by id. 404, like a missing one.
    scope::deny_out_of_scope(&db, &actor, invitation.org_id).await?;

    // The path names a user, so refuse to act on an invitation belonging to a
    // different address — a mistyped id must 404, not revoke someone else's.
    // `user.email`, never a label: this is an ownership check, and a label
    // falls back to the non-unique `name`. A user with no address owns no
    // invitations, so they cannot revoke one.
    let Some(user_email) = user.email.as_deref() else {
        return Err(StatusCode::NOT_FOUND);
    };
    if !invitation.email.eq_ignore_ascii_case(user_email.trim()) {
        return Err(StatusCode::NOT_FOUND);
    }

    let org_id = invitation.org_id;
    let target_email = invitation.email.clone();
    let active: org_invitations::ActiveModel = invitation.into();
    active.delete(&db).await.map_err(internal)?;

    tracing::info!(
        admin_email = %actor.label(),
        target_id = %user_id,
        target_email = %target_email,
        org_id = %org_id,
        action = "revoke_invitation",
        "admin tenant action"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn lookup_org_slug<C>(db: &C, org_id: Uuid) -> Option<String>
where
    C: sea_orm::ConnectionTrait,
{
    organizations::Entity::find_by_id(org_id)
        .one(db)
        .await
        .ok()
        .flatten()
        .map(|o| o.slug)
}

async fn load_user_org_memberships(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<UserOrgMembership>, sea_orm::DbErr> {
    let rows = org_members::Entity::find()
        .filter(org_members::Column::UserId.eq(user_id))
        .order_by_asc(org_members::Column::CreatedAt)
        .all(db)
        .await?;

    // Batch the org lookups into a single query instead of one per row.
    let org_ids: Vec<Uuid> = rows.iter().map(|m| m.org_id).collect();
    let orgs_by_id: HashMap<Uuid, organizations::Model> = organizations::Entity::find()
        .filter(organizations::Column::Id.is_in(org_ids))
        .all(db)
        .await?
        .into_iter()
        .map(|o| (o.id, o))
        .collect();

    let mut out = Vec::with_capacity(rows.len());
    for m in rows {
        if let Some(org) = orgs_by_id.get(&m.org_id) {
            out.push(UserOrgMembership {
                org_id: org.id,
                org_slug: org.slug.clone(),
                org_name: org.name.clone(),
                role: m.role.as_str().to_string(),
                joined_at: m.created_at.to_rfc3339(),
            });
        }
    }
    Ok(out)
}

async fn load_user_workspace_memberships(
    db: &sea_orm::DatabaseConnection,
    user_id: Uuid,
) -> Result<Vec<UserWorkspaceMembership>, sea_orm::DbErr> {
    let rows = workspace_members::Entity::find()
        .filter(workspace_members::Column::UserId.eq(user_id))
        .order_by_asc(workspace_members::Column::CreatedAt)
        .all(db)
        .await?;

    // Batch the workspace lookups into a single query instead of one per row.
    let workspace_ids: Vec<Uuid> = rows.iter().map(|m| m.workspace_id).collect();
    let workspaces_by_id: HashMap<Uuid, entity::workspaces::Model> =
        entity::workspaces::Entity::find()
            .filter(entity::workspaces::Column::Id.is_in(workspace_ids))
            .all(db)
            .await?
            .into_iter()
            .map(|ws| (ws.id, ws))
            .collect();

    let mut out = Vec::with_capacity(rows.len());
    for m in rows {
        if let Some(ws) = workspaces_by_id.get(&m.workspace_id) {
            out.push(UserWorkspaceMembership {
                workspace_id: ws.id,
                workspace_name: ws.name.clone(),
                role: m.role.as_str().to_string(),
                joined_at: m.created_at.to_rfc3339(),
            });
        }
    }
    Ok(out)
}

fn internal<E: std::fmt::Display>(e: E) -> StatusCode {
    tracing::error!("users_admin internal error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

fn internal_resp<E: std::fmt::Display>(e: E) -> Response {
    tracing::error!("users_admin internal error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR.into_response()
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

fn error_body(status: StatusCode, code: &'static str, message: Option<String>) -> Response {
    (status, Json(ErrorBody { code, message })).into_response()
}

/// Pure helper that captures the last-owner guard policy used by both
/// `update_role` and `remove_from_org`. Returns `true` when the operation
/// would strip the org of its last Owner and must be rejected.
///
/// - `current_owners` — distinct Owner user ids on the org right now.
/// - `target_user_id` — the user being demoted or removed.
/// - `new_role` — `Some(role)` for an update, `None` for a removal.
fn would_strand_owner(
    current_owners: &[Uuid],
    target_user_id: Uuid,
    new_role: Option<&OrgRole>,
) -> bool {
    let target_is_owner = current_owners.contains(&target_user_id);
    if !target_is_owner {
        return false;
    }
    let demoting = match new_role {
        Some(OrgRole::Owner) => false,
        Some(_) | None => true,
    };
    demoting && current_owners.len() <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_status_body_parses_active() {
        let parsed = UserStatus::from_str("active");
        assert!(parsed.is_ok());
        assert!(matches!(parsed.unwrap(), UserStatus::Active));
    }

    #[test]
    fn set_status_body_parses_deleted() {
        let parsed = UserStatus::from_str("deleted");
        assert!(parsed.is_ok());
        assert!(matches!(parsed.unwrap(), UserStatus::Deleted));
    }

    #[test]
    fn set_status_body_rejects_unknown() {
        assert!(UserStatus::from_str("suspended").is_err());
    }

    #[test]
    fn add_to_org_rejects_unknown_role() {
        assert!(OrgRole::from_str("not-a-role").is_err());
    }

    #[test]
    fn add_to_org_accepts_owner_admin_member() {
        assert!(matches!(
            OrgRole::from_str("owner").unwrap(),
            OrgRole::Owner
        ));
        assert!(matches!(
            OrgRole::from_str("admin").unwrap(),
            OrgRole::Admin
        ));
        assert!(matches!(
            OrgRole::from_str("member").unwrap(),
            OrgRole::Member
        ));
    }

    // ---------------- last-owner guard ----------------

    #[test]
    fn last_owner_guard_blocks_demote_when_only_owner() {
        let only_owner = Uuid::new_v4();
        let owners = vec![only_owner];
        assert!(
            would_strand_owner(&owners, only_owner, Some(&OrgRole::Admin)),
            "must block demote to Admin when only one owner exists"
        );
        assert!(
            would_strand_owner(&owners, only_owner, Some(&OrgRole::Member)),
            "must block demote to Member when only one owner exists"
        );
    }

    #[test]
    fn last_owner_guard_blocks_remove_when_only_owner() {
        let only_owner = Uuid::new_v4();
        let owners = vec![only_owner];
        assert!(
            would_strand_owner(&owners, only_owner, None),
            "must block remove when only one owner exists"
        );
    }

    #[test]
    fn last_owner_guard_allows_demote_when_fallback_owner_exists() {
        let target = Uuid::new_v4();
        let other = Uuid::new_v4();
        let owners = vec![target, other];
        assert!(
            !would_strand_owner(&owners, target, Some(&OrgRole::Admin)),
            "must allow demote when another Owner remains"
        );
        assert!(
            !would_strand_owner(&owners, target, None),
            "must allow remove when another Owner remains"
        );
    }

    #[test]
    fn last_owner_guard_ignores_non_owner_target() {
        let owner = Uuid::new_v4();
        let target = Uuid::new_v4();
        let owners = vec![owner];
        assert!(
            !would_strand_owner(&owners, target, Some(&OrgRole::Admin)),
            "non-owner demote must never be blocked"
        );
        assert!(
            !would_strand_owner(&owners, target, None),
            "non-owner remove must never be blocked"
        );
    }

    #[test]
    fn last_owner_guard_allows_no_op_promote_to_owner() {
        let only_owner = Uuid::new_v4();
        let owners = vec![only_owner];
        assert!(
            !would_strand_owner(&owners, only_owner, Some(&OrgRole::Owner)),
            "must allow updating the last owner to Owner (no-op)"
        );
    }
}

#[derive(FromQueryResult)]
struct PartnerAdminRow {
    user_id: Uuid,
    partner_id: Uuid,
    partner_name: String,
}

/// Which partners each user holds a role at, and which role.
///
/// Keyed on `user_id`, not email: the old `partner_members` table keyed grants by
/// email so they could precede first sign-in, and left `user_id` NULL forever.
/// That duplicate membership system is gone — a partner's people are members of
/// the partner org, and org invitations already cover the not-yet-signed-up case.
///
/// The chain is: the user is an `org_members` row in an org that holds a
/// `partner_grant`, AND that membership carries a `partner_role_binding`. A member
/// of Acme with no binding is just an Acme employee, so they correctly get nothing.
async fn lookup_partner_admins_in(
    db: &sea_orm::DatabaseConnection,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, Vec<UserPartnerRef>>, sea_orm::DbErr> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = sql_placeholders(user_ids.len());
    let sql = format!(
        "SELECT om.user_id, o.id AS partner_id, o.name AS partner_name \
         FROM org_members om \
         JOIN partner_role_bindings prb ON prb.org_member_id = om.id \
         JOIN partner_grants pg ON pg.org_id = om.org_id \
         JOIN organizations o ON o.id = om.org_id \
         WHERE om.user_id IN ({placeholders}) AND pg.status = 'active'"
    );
    let values: Vec<sea_orm::Value> = user_ids.iter().map(|id| (*id).into()).collect();
    let rows = PartnerAdminRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;

    let mut out: HashMap<Uuid, Vec<UserPartnerRef>> = HashMap::new();
    for r in rows {
        out.entry(r.user_id).or_default().push(UserPartnerRef {
            id: r.partner_id,
            name: r.partner_name,
        });
    }
    Ok(out)
}

#[derive(FromQueryResult)]
struct TopRoleRow {
    user_id: Uuid,
    role: String,
}

/// Each user's HIGHEST org role (owner > admin > member) — the tenant hierarchy
/// the directory was missing. Ordered in SQL so one query answers it for a page.
async fn lookup_top_org_role_in(
    db: &sea_orm::DatabaseConnection,
    user_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, sea_orm::DbErr> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = sql_placeholders(user_ids.len());
    let sql = format!(
        "SELECT DISTINCT ON (user_id) user_id, role FROM org_members \
         WHERE user_id IN ({placeholders}) \
         ORDER BY user_id, CASE role \
           WHEN 'owner' THEN 0 WHEN 'admin' THEN 1 ELSE 2 END"
    );
    let values: Vec<sea_orm::Value> = user_ids.iter().map(|id| (*id).into()).collect();
    let rows = TopRoleRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.user_id, r.role)).collect())
}
