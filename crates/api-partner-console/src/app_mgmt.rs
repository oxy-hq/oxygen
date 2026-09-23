//! Partner custom-app management for `/api/partners/{id}/...`.
//!
//! The `manage_apps` capability: a partner admin can see the custom apps in a
//! managed org and control their published (live-to-viewers) state. Every
//! handler passes through [`require_org_scope`] with `ManageApps`, and
//! app-centric routes additionally confirm the app's org is one the partner
//! manages (via [`load_managed_app`]) before acting.
//!
//! Publish/unpublish are pointer/flag moves on the `apps` row (the bytes already
//! live in S3). Build-pointer rollback/promote-latest reuse the heavier admin
//! build logic and land in a follow-up — through the same gate.

use axum::Json;
use axum::extract::Path;
use axum::http::StatusCode;
use entity::apps;
use entity::prelude::Apps;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use super::{db, internal, require_org_scope};
use crate::partner_context::PartnerActor;
use oxy_app::server::api::admin::apps::handlers as admin_apps;
use oxy_app::server::api::org_teams::dto::{
    AppAccessDto, AppAccessSummaryDto, OrgMemberOptionDto, SetAppAccessRequest, TeamDto,
};
use oxy_app::server::api::org_teams::service as access_service;
use oxy_app_core::audit::{self, ActorType, AuditEntry};
use oxy_server_authz::partner_authz::{PartnerCapability, PartnerScope};

#[derive(Serialize)]
pub struct PartnerAppDto {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub published: bool,
}

impl From<apps::Model> for PartnerAppDto {
    fn from(a: apps::Model) -> Self {
        Self {
            published: a.published_at.is_some(),
            id: a.id,
            slug: a.slug,
            name: a.name,
        }
    }
}

/// `GET /partners/{id}/orgs/{org_id}/apps` — custom apps in a managed org.
///
/// Returns the same access-bearing shape as the partner's OWN app list, so the
/// console can show visibility and grant count on both. A `manage_apps` partner is a
/// plausible author of the state the badge exists to surface — an app opened to the
/// whole org while admin grants survive — so this is the surface where it is most
/// load-bearing, and it was the one list with no access indicator at all.
pub async fn list_org_apps(
    PartnerActor(scope): PartnerActor,
    Path((_partner_id, org_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<AppAccessSummaryDto>>, StatusCode> {
    let db = db().await?;
    require_org_scope(&db, &scope, org_id, PartnerCapability::ManageApps).await?;
    Ok(Json(
        access_service::list_org_apps_with_access(&db, org_id).await?,
    ))
}

/// Load an app and confirm the partner manages its org (with `ManageApps`).
/// Returns `404` for a missing app or one outside the partner's subtree.
async fn load_managed_app(
    db: &sea_orm::DatabaseConnection,
    scope: &PartnerScope,
    app_id: Uuid,
) -> Result<apps::Model, StatusCode> {
    let app = Apps::find_by_id(app_id)
        .one(db)
        .await
        .map_err(internal("load app"))?
        .ok_or(StatusCode::NOT_FOUND)?;
    require_org_scope(db, scope, app.org_id, PartnerCapability::ManageApps).await?;
    Ok(app)
}

/// Load an app the caller may manage from this console — either a **client's**
/// (partner ceiling) or the partner org's **own** (org authority).
///
/// The distinction is the whole point. A partner is a real org with its own apps,
/// and those were unreachable here because `scope.org_ids` is the managed-client
/// set and never contains the partner itself. But routing the own-org case through
/// the ceiling would CREATE privilege: every operator holds the partner's whole
/// ceiling, so a `manage_apps` operator who is not an officer of their own org
/// would gain app management over it. So the own-org branch is authorized by
/// `Action::AppAccessManage` against the org — exactly the authority the org's own
/// settings page requires — and grants nothing new.
async fn load_manageable_app(
    db: &sea_orm::DatabaseConnection,
    scope: &PartnerScope,
    actor: &oxy_auth::types::AuthenticatedUser,
    app_id: Uuid,
) -> Result<apps::Model, StatusCode> {
    let app = Apps::find_by_id(app_id)
        .one(db)
        .await
        .map_err(internal("load app"))?
        .ok_or(StatusCode::NOT_FOUND)?;

    if app.org_id == scope.partner_id {
        require_own_org_authority(db, actor, app.org_id).await?;
    } else {
        require_org_scope(db, scope, app.org_id, PartnerCapability::ManageApps).await?;
    }
    Ok(app)
}

/// The caller is an officer of `org_id` in their own right — either a REAL
/// owner/admin membership, or a live assume-role session over `org_id`.
///
/// `existing_allow` is that fact, not a hand-waved `true`, so the decision
/// stays the conjunction the authz crate is built on and the model can only
/// subtract.
///
/// The assume-role half matters because it is the ONLY way Oxy staff reach
/// this console at all (`PartnerActor`'s extraction already proved the caller
/// is a real operator or holds a live assume session over the partner org —
/// see `resolve_scope`'s "Real operator first; failing that, a live assume
/// session"), and an assume session deliberately never creates a real
/// `org_members` row (this module's own doc comment: "never injected as a
/// synthetic membership"). Without this, `is_officer` was always false for
/// every staff-driven visit, and `AssumeRoleDialog` explicitly promises
/// "Owner-level access to this organization" for the session's duration.
/// `is_session_live` is the same enforcement primitive `org_context` /
/// `workspace_context` use before synthesizing an Owner membership, and it
/// fails closed on a DB error like the membership read below.
async fn require_own_org_authority(
    db: &sea_orm::DatabaseConnection,
    actor: &oxy_auth::types::AuthenticatedUser,
    org_id: Uuid,
) -> Result<(), StatusCode> {
    use entity::org_members;
    let is_real_officer = entity::prelude::OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(actor.id))
        .filter(
            org_members::Column::Role
                .is_in([org_members::OrgRole::Owner, org_members::OrgRole::Admin]),
        )
        .one(db)
        .await
        .map_err(internal("check own-org role"))?
        .is_some();
    let is_officer = is_real_officer
        || oxy_server_authz::assume_liveness::is_session_live(db, actor.id, org_id).await;

    let allowed = oxy_server_authz::enforce_for(
        db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        "partner_console.own_app",
        oxy_authz::Action::AppAccessManage,
        oxy_authz::Resource::org(org_id),
        is_officer,
    )
    .await;
    if allowed {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// `GET /partners/{id}/own-apps` — the partner org's OWN custom apps.
///
/// Separate from `/orgs/{org_id}/apps` because the partner org is not one of its own
/// clients; listing it there would mean putting it in `scope.org_ids`, which is the
/// ceiling's reach. Kept apart so the console can show "your apps" beside "your
/// clients' apps" without the two sharing an authority.
pub async fn list_own_apps(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(_partner_id): Path<Uuid>,
) -> Result<Json<Vec<AppAccessSummaryDto>>, StatusCode> {
    let db = db().await?;
    require_own_org_authority(&db, &actor, scope.partner_id).await?;
    Ok(Json(
        access_service::list_org_apps_with_access(&db, scope.partner_id).await?,
    ))
}

/// `GET /partners/{id}/own-teams` — the partner org's own teams, so the grant picker
/// works for its own apps too.
pub async fn list_own_teams(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(_partner_id): Path<Uuid>,
) -> Result<Json<Vec<TeamDto>>, StatusCode> {
    let db = db().await?;
    require_own_org_authority(&db, &actor, scope.partner_id).await?;
    Ok(Json(
        access_service::list_org_teams(&db, scope.partner_id).await?,
    ))
}

/// `GET /partners/{id}/own-people` — the partner org's own people, for the picker.
pub async fn list_own_people(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(_partner_id): Path<Uuid>,
) -> Result<Json<Vec<OrgMemberOptionDto>>, StatusCode> {
    let db = db().await?;
    require_own_org_authority(&db, &actor, scope.partner_id).await?;
    Ok(Json(
        access_service::list_org_member_options(&db, scope.partner_id).await?,
    ))
}

/// `GET /partners/{id}/apps/{app_id}/access` — who may open a managed app.
///
/// Gated by `ManageApps`, the same capability as publish/unpublish: naming an app's
/// audience is lifecycle. Deliberately NOT `DevelopApps` — a partner can decide who
/// sees the app without being able to read the app's data, which is the split
/// `Ring::AppAccess` keeps on the other side.
pub async fn get_app_access(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_id, app_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AppAccessDto>, StatusCode> {
    let db = db().await?;
    let app = load_manageable_app(&db, &scope, &actor, app_id).await?;
    Ok(Json(access_service::read_access(&db, &app).await?))
}

/// `PUT /partners/{id}/apps/{app_id}/access`
pub async fn set_app_access(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_id, app_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<SetAppAccessRequest>,
) -> Result<Json<AppAccessDto>, StatusCode> {
    let db = db().await?;
    let app = load_manageable_app(&db, &scope, &actor, app_id).await?;
    let (org_id, name, slug) = (app.org_id, app.name.clone(), app.slug.clone());

    let out = access_service::write_access(&db, &app, actor.id, &req).await?;

    // Every partner action lands in the client org's append-only log — this one
    // especially, since it changes who can see the client's data.
    audit::record_best_effort(
        &db,
        AuditEntry::new(actor.label().to_string(), "partner.app.access_changed")
            .actor(actor.id, ActorType::PartnerAdmin)
            .partner(scope.partner_id)
            .org(org_id)
            .target("app", app_id.to_string(), format!("{name} ({slug})")),
    )
    .await;
    Ok(Json(out))
}

/// `GET /partners/{id}/orgs/{org_id}/teams` — the client org's teams, so the
/// console can offer the same grant picker the org itself sees.
pub async fn list_org_teams(
    PartnerActor(scope): PartnerActor,
    Path((_partner_id, org_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<TeamDto>>, StatusCode> {
    let db = db().await?;
    require_org_scope(&db, &scope, org_id, PartnerCapability::ManageApps).await?;
    Ok(Json(access_service::list_org_teams(&db, org_id).await?))
}

/// `GET /partners/{id}/orgs/{org_id}/grantable-people` — the client org's people,
/// for the individual half of the grant picker.
///
/// Gated on **`ManageApps`**, deliberately not the existing `list_org_members`
/// (`/orgs/{id}/members`), which requires `ManageMembers`. Those are different
/// capabilities and a `manage_apps`-only partner is the *expected* configuration for
/// this console, not an edge case — routing the picker through the member-management
/// endpoint gave them a silent 403, so the picker showed teams only and any existing
/// individual grants rendered as "Unknown person" and could be saved back under that
/// label.
///
/// The payload is the grantee-picker projection (id, email, name, org role) — the
/// same shape the org's own settings and the admin console use, and strictly less
/// than `list_org_members` returns.
pub async fn list_grantable_people(
    PartnerActor(scope): PartnerActor,
    Path((_partner_id, org_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Vec<OrgMemberOptionDto>>, StatusCode> {
    let db = db().await?;
    require_org_scope(&db, &scope, org_id, PartnerCapability::ManageApps).await?;
    Ok(Json(
        access_service::list_org_member_options(&db, org_id).await?,
    ))
}

/// `POST /partners/{id}/apps/{app_id}/publish` — make the app live to viewers.
pub async fn publish_app(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_id, app_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<PartnerAppDto>, StatusCode> {
    let db = db().await?;
    let app = load_managed_app(&db, &scope, app_id).await?;
    let org_id = app.org_id;
    let (name, slug) = (app.name.clone(), app.slug.clone());

    // Reuse the canonical publish so the partner path can't drift from admin:
    // it repoints published_build_id at the draft, stamps last_promoted_*, and
    // drops the per-app canonical-dir caches. Re-stamping published_at by hand
    // would publish a channel with no live bytes behind it.
    let saved = admin_apps::publish_one(&db, app_id, actor.id)
        .await
        .map_err(|e| e.status)?;
    // Viewers' cached access must drop now, not at TTL (admin does the same).
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();

    audit::record_best_effort(
        &db,
        AuditEntry::new(actor.label().to_string(), "partner.app.published")
            .actor(actor.id, ActorType::PartnerAdmin)
            .partner(scope.partner_id)
            .org(org_id)
            .target("app", app_id.to_string(), format!("{name} ({slug})")),
    )
    .await;
    Ok(Json(saved.into()))
}

/// `DELETE /partners/{id}/apps/{app_id}/publish` — take the app out of view.
pub async fn unpublish_app(
    PartnerActor(scope): PartnerActor,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_partner_id, app_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<PartnerAppDto>, StatusCode> {
    let db = db().await?;
    let app = load_managed_app(&db, &scope, app_id).await?;
    let org_id = app.org_id;
    let (name, slug) = (app.name.clone(), app.slug.clone());

    // Canonical unpublish: also nulls published_build_id (no dangling pointer)
    // and drops the per-app canonical-dir caches.
    let saved = admin_apps::unpublish_one(&db, app_id)
        .await
        .map_err(|e| e.status)?;
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();

    audit::record_best_effort(
        &db,
        AuditEntry::new(actor.label().to_string(), "partner.app.unpublished")
            .actor(actor.id, ActorType::PartnerAdmin)
            .partner(scope.partner_id)
            .org(org_id)
            .target("app", app_id.to_string(), format!("{name} ({slug})")),
    )
    .await;
    Ok(Json(saved.into()))
}
