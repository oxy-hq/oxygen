//! `/orgs/{org_id}/teams/*` — the org's team roster.
//!
//! Teams exist to be granted app access ([`super::app_access`]), so every handler
//! here is gated by the same authority: [`Action::AppAccessManage`]. The
//! [`OrgAdmin`] extractor is the shipped guard and this module's `existing_allow`;
//! the `enforce_*` call narrows it — `OrgAdmin` admits ANY managing partner, while
//! `AppAccessManage` admits only one holding `manage_apps`. The decision is the
//! conjunction, so the model can only subtract.

use axum::{Json, extract::Path, http::StatusCode};
use entity::prelude::{OrgMembers, OrgTeamMembers, OrgTeams, Users};
use entity::{org_members, org_team_members, org_teams, users};
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_authz::{Action, Resource};
use oxy_server_authz::role_guards::OrgAdmin;
use sea_orm::{
    ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter,
};
use std::collections::HashMap;
use uuid::Uuid;

use super::audit;
use super::dto::{
    AddTeamMemberRequest, CreateTeamRequest, TeamDetailDto, TeamDto, TeamMemberDto,
    UpdateTeamRequest,
};
use super::service;

/// Max teams per org. Not a product limit anyone should hit — a backstop so a
/// runaway client can't turn the grant picker into an unbounded list.
const MAX_TEAMS_PER_ORG: u64 = 500;

fn db_err(e: impl std::fmt::Display) -> StatusCode {
    tracing::error!("org_teams: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

/// The gate every handler in this module shares.
///
/// `OrgAdmin` already ran as an extractor — that IS the `existing_allow` term. This
/// narrows it to [`Action::AppAccessManage`], whose partner term is `manage_apps`
/// specifically rather than `OrgAdmin`'s any-managing-partner.
pub(super) async fn enforce_team_manage(
    db: &DatabaseConnection,
    actor_id: Uuid,
    actor_email: &str,
    org_id: Uuid,
) -> Result<(), StatusCode> {
    let allowed = oxy_server_authz::enforce_for(
        db,
        actor_id,
        actor_email,
        "org_teams.manage",
        Action::AppAccessManage,
        Resource::org(org_id),
        // The OrgAdmin extractor is the shipped check; reaching here means it passed.
        true,
    )
    .await;
    if allowed {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

/// Normalized team name, or `None` when it is blank. Names are stored as typed but
/// compared case-insensitively (a `lower(name)` unique index backs this).
fn normalize_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 100 {
        return None;
    }
    Some(trimmed.to_string())
}

/// `GET /orgs/{org_id}/teams`
pub async fn list_teams(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
) -> Result<Json<Vec<TeamDto>>, StatusCode> {
    let db = establish_connection().await.map_err(db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;

    Ok(Json(service::list_org_teams(&db, ctx.org.id).await?))
}

/// `POST /orgs/{org_id}/teams`
pub async fn create_team(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Json(req): Json<CreateTeamRequest>,
) -> Result<Json<TeamDto>, StatusCode> {
    let db = establish_connection().await.map_err(db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;

    let name = normalize_name(&req.name).ok_or(StatusCode::BAD_REQUEST)?;

    let existing = OrgTeams::find()
        .filter(org_teams::Column::OrgId.eq(ctx.org.id))
        .count(&db)
        .await
        .map_err(db_err)?;
    if existing >= MAX_TEAMS_PER_ORG {
        return Err(StatusCode::CONFLICT);
    }

    let model = org_teams::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(ctx.org.id),
        name: ActiveValue::Set(name),
        description: ActiveValue::Set(req.description.filter(|d| !d.trim().is_empty())),
        created_by: ActiveValue::Set(Some(actor.id)),
        created_at: ActiveValue::NotSet,
        updated_at: ActiveValue::NotSet,
    };
    // A duplicate name trips the `lower(name)` unique index — reported as 409 rather
    // than a 500, since it is a user-correctable collision, not a server fault.
    let team = OrgTeams::insert(model)
        .exec_with_returning(&db)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                StatusCode::CONFLICT
            } else {
                db_err(e)
            }
        })?;

    audit::record(
        &db,
        &ctx,
        &actor,
        audit::TEAM_CREATED,
        ("team", team.id, team.name.clone()),
    )
    .await;
    Ok(Json(service::to_team_dto(team, &HashMap::new())))
}

/// True for a Postgres unique-violation (SQLSTATE **23505**).
///
/// Matched on the code, not on the message text: the English string
/// ("duplicate key value") moves with driver version and server locale, and a false
/// negative here is not cosmetic — it turns a name collision's 409 into a 500, and
/// turns `add_team_member`'s deliberately-idempotent re-add into an error.
fn is_unique_violation(e: &sea_orm::DbErr) -> bool {
    use sea_orm::{DbErr, RuntimeErr};
    let sqlx_err = match e {
        DbErr::Query(RuntimeErr::SqlxError(e))
        | DbErr::Exec(RuntimeErr::SqlxError(e))
        | DbErr::Conn(RuntimeErr::SqlxError(e)) => e,
        _ => return false,
    };
    sqlx_err
        .as_database_error()
        .and_then(|d| d.code())
        .is_some_and(|code| code == "23505")
}

/// `GET /orgs/{org_id}/teams/{team_id}`
pub async fn get_team(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, team_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<TeamDetailDto>, StatusCode> {
    let db = establish_connection().await.map_err(db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;
    let team = load_team(&db, ctx.org.id, team_id).await?;

    let rows = OrgTeamMembers::find()
        .filter(org_team_members::Column::TeamId.eq(team_id))
        .find_also_related(Users)
        .all(&db)
        .await
        .map_err(db_err)?;

    // One query for the org roles of everyone in the team, so the panel can show
    // "Alice — Admin" without a lookup per row.
    let user_ids: Vec<Uuid> = rows.iter().map(|(m, _)| m.user_id).collect();
    let roles = org_roles(&db, ctx.org.id, user_ids).await?;

    let mut members: Vec<TeamMemberDto> = rows
        .into_iter()
        .filter_map(|(m, user)| {
            let user: users::Model = user?;
            Some(TeamMemberDto {
                org_role: roles
                    .get(&m.user_id)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string()),
                user_id: m.user_id,
                name: service::display_name(&user),
                email: user.label().to_string(),
                added_at: m.created_at.to_rfc3339(),
            })
        })
        .collect();
    members.sort_by(|a, b| a.email.cmp(&b.email));

    let counts = HashMap::from([(team_id, members.len() as u64)]);
    Ok(Json(TeamDetailDto {
        team: service::to_team_dto(team, &counts),
        members,
    }))
}

async fn org_roles(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_ids: Vec<Uuid>,
) -> Result<HashMap<Uuid, String>, StatusCode> {
    if user_ids.is_empty() {
        return Ok(HashMap::new());
    }
    Ok(OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.is_in(user_ids))
        .all(db)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|m| (m.user_id, m.role.as_str().to_string()))
        .collect())
}

/// Load a team, 404ing when it belongs to another org. The org filter is the
/// tenant boundary — a team id alone must never resolve across orgs.
pub(super) async fn load_team(
    db: &DatabaseConnection,
    org_id: Uuid,
    team_id: Uuid,
) -> Result<org_teams::Model, StatusCode> {
    OrgTeams::find_by_id(team_id)
        .filter(org_teams::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or(StatusCode::NOT_FOUND)
}

/// `PATCH /orgs/{org_id}/teams/{team_id}`
pub async fn update_team(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, team_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<UpdateTeamRequest>,
) -> Result<Json<TeamDto>, StatusCode> {
    let db = establish_connection().await.map_err(db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;
    load_team(&db, ctx.org.id, team_id).await?;

    let name = normalize_name(&req.name).ok_or(StatusCode::BAD_REQUEST)?;
    let model = org_teams::ActiveModel {
        id: ActiveValue::Unchanged(team_id),
        name: ActiveValue::Set(name),
        description: ActiveValue::Set(req.description.filter(|d| !d.trim().is_empty())),
        updated_at: ActiveValue::Set(chrono::Utc::now().into()),
        ..Default::default()
    };
    let team = OrgTeams::update(model).exec(&db).await.map_err(|e| {
        if is_unique_violation(&e) {
            StatusCode::CONFLICT
        } else {
            db_err(e)
        }
    })?;

    audit::record(
        &db,
        &ctx,
        &actor,
        audit::TEAM_UPDATED,
        ("team", team_id, team.name.clone()),
    )
    .await;
    let counts = service::team_member_counts(&db, vec![team_id]).await?;
    Ok(Json(service::to_team_dto(team, &counts)))
}

/// `DELETE /orgs/{org_id}/teams/{team_id}`
///
/// `app_team_grants` and `org_team_members` cascade, so deleting a team revokes
/// every app grant it carried. That is the intent — but it means the access cache
/// has to be dropped, or a deleted team keeps working for up to its TTL.
pub async fn delete_team(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, team_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;
    // Read the name BEFORE the delete: the audit row is the only place it survives,
    // and a log entry saying a uuid lost its grants is not a log entry anyone reads.
    let team = load_team(&db, ctx.org.id, team_id).await?;

    OrgTeams::delete_by_id(team_id)
        .exec(&db)
        .await
        .map_err(db_err)?;
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();
    audit::record(
        &db,
        &ctx,
        &actor,
        audit::TEAM_DELETED,
        ("team", team_id, team.name),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /orgs/{org_id}/teams/{team_id}/members`
///
/// The grantee-boundary check lives here: only an org member may join a team. It is
/// enforced independently by `Ring::AppAccess`'s org-membership term, so this is
/// defense in depth — but rejecting at write time is what makes the rule visible to
/// the person doing the adding instead of silently producing a grant that does
/// nothing.
pub async fn add_team_member(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, team_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<AddTeamMemberRequest>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;
    let team = load_team(&db, ctx.org.id, team_id).await?;

    let is_member = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(ctx.org.id))
        .filter(org_members::Column::UserId.eq(req.user_id))
        .one(&db)
        .await
        .map_err(db_err)?
        .is_some();
    if !is_member {
        return Err(StatusCode::BAD_REQUEST);
    }

    let model = org_team_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        team_id: ActiveValue::Set(team_id),
        user_id: ActiveValue::Set(req.user_id),
        created_by: ActiveValue::Set(Some(actor.id)),
        created_at: ActiveValue::NotSet,
    };
    match OrgTeamMembers::insert(model).exec(&db).await {
        Ok(_) => {}
        // Already in the team — idempotent, not an error. No audit row either: the
        // log records changes, and a re-POST of an existing membership is not one.
        Err(e) if is_unique_violation(&e) => return Ok(StatusCode::NO_CONTENT),
        Err(e) => return Err(db_err(e)),
    }
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();
    audit::record(
        &db,
        &ctx,
        &actor,
        audit::TEAM_MEMBER_ADDED,
        ("team", team_id, format!("{} + {}", team.name, req.user_id)),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

/// `DELETE /orgs/{org_id}/teams/{team_id}/members/{user_id}`
pub async fn remove_team_member(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((_org_id, team_id, user_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(db_err)?;
    enforce_team_manage(
        &db,
        actor.id,
        actor.email.as_deref().unwrap_or(""),
        ctx.org.id,
    )
    .await?;
    let team = load_team(&db, ctx.org.id, team_id).await?;

    OrgTeamMembers::delete_many()
        .filter(org_team_members::Column::TeamId.eq(team_id))
        .filter(org_team_members::Column::UserId.eq(user_id))
        .exec(&db)
        .await
        .map_err(db_err)?;
    oxy_app::server::api::custom_apps_auth::invalidate_access_cache();
    audit::record(
        &db,
        &ctx,
        &actor,
        audit::TEAM_MEMBER_REMOVED,
        ("team", team_id, format!("{} - {}", team.name, user_id)),
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_oversized_names_are_rejected() {
        assert_eq!(normalize_name("  Finance "), Some("Finance".to_string()));
        assert_eq!(normalize_name("   "), None);
        assert_eq!(normalize_name(""), None);
        assert_eq!(normalize_name(&"a".repeat(101)), None);
        assert_eq!(normalize_name(&"a".repeat(100)).map(|s| s.len()), Some(100));
    }
}
