//! `/api/admin/workspaces/*` — OXY_OWNER-only meta surface for workspaces
//! across every organization. Sits behind `oxy_owner_guard_middleware`.

use std::collections::HashMap;

use axum::extract::{OriginalUri, Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use chrono::Utc;
use entity::workspaces::WorkspaceStatus;
use entity::{organizations, users, workspace_members, workspaces};
use oxy::database::client::establish_connection;
use oxy_app_core::pagination::{self, Paged, trim_overfetch};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, DatabaseBackend, EntityTrait, FromQueryResult, PaginatorTrait,
    QueryFilter, QueryOrder, QuerySelect, Set, Statement,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use oxy_app::surface::admin_scope as scope;
use oxy_app_core::AppState;

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/workspaces-meta", get(list_workspaces))
        .route(
            "/workspaces/{workspace_id}/detail",
            get(get_workspace_detail),
        )
        .route(
            "/workspaces/{workspace_id}",
            patch(update_workspace).delete(delete_workspace),
        )
        .route(
            "/workspaces/{workspace_id}/transfer-org",
            post(transfer_org),
        )
}

#[derive(Serialize)]
pub struct AdminWorkspaceRow {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub last_opened_at: Option<String>,
    pub org_id: Option<Uuid>,
    pub org_slug: Option<String>,
    pub member_count: i64,
}

#[derive(Serialize)]
pub struct AdminWorkspaceDetail {
    pub id: Uuid,
    pub name: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    pub last_opened_at: Option<String>,
    pub org_id: Option<Uuid>,
    pub org_slug: Option<String>,
    pub org_name: Option<String>,
    pub path: Option<String>,
    pub git_remote_url: Option<String>,
    pub error: Option<String>,
    pub member_count: i64,
    pub members: Vec<WorkspaceMember>,
    /// Last promoted compile revision (when set). The admin compiles
    /// page links to this from the workspace detail view.
    pub current_revision_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct WorkspaceMember {
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    pub role: String,
    pub joined_at: String,
}

#[derive(Deserialize)]
pub struct ListWorkspacesQuery {
    pub search: Option<String>,
    pub status: Option<String>,
    pub org_id: Option<Uuid>,
    pub page: Option<u64>,
    pub page_size: Option<u64>,
}

#[derive(Deserialize)]
pub struct UpdateWorkspaceBody {
    pub name: Option<String>,
}

#[derive(Deserialize)]
pub struct TransferOrgBody {
    pub new_org_id: Uuid,
}

/// `GET /admin/workspaces-meta` — the workspace directory.
///
/// **Narrowed by the caller's grant scope, in the query**, ahead of the paging: a
/// bounded grant lists the workspaces of the orgs it names. A workspace with no org
/// is platform-level — unbounded grants and the Global Owner only — which is the
/// answer `get_workspace_detail` already gives for the same row.
pub async fn list_workspaces(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    OriginalUri(uri): OriginalUri,
    Query(q): Query<ListWorkspacesQuery>,
) -> Result<Paged<AdminWorkspaceRow>, StatusCode> {
    let db = establish_connection().await.map_err(internal)?;
    let reach = scope::list_scope(&db, &actor).await?;
    let page = q.page.unwrap_or(0);
    // CLAMPED AT BOTH ENDS. `?page_size=0` past a top-only clamp is an infinite
    // pagination loop, not an empty page: the offset stays 0 on every page while
    // `page + 1` keeps advancing, so every request answers `[]` with a link to
    // the next one. See `oxy_app_core::pagination`.
    let page_size = q.page_size.unwrap_or(50).clamp(1, 200);

    let mut query = workspaces::Entity::find().order_by_desc(workspaces::Column::CreatedAt);
    if let Some(orgs) = reach {
        query = query.filter(workspaces::Column::OrgId.is_in(orgs));
    }

    if let Some(needle) = q.search.as_ref().filter(|s| !s.trim().is_empty()) {
        let like = format!("%{}%", needle.trim());
        query = query.filter(workspaces::Column::Name.contains(like.as_str()));
    }
    if let Some(status_str) = q.status.as_ref().filter(|s| !s.is_empty()) {
        let status = parse_status(status_str)?;
        query = query.filter(workspaces::Column::Status.eq(status));
    }
    if let Some(org_id) = q.org_id {
        query = query.filter(workspaces::Column::OrgId.eq(org_id));
    }

    // `page_size + 1`: the extra row is how the `Link: rel="next"` below knows a
    // next page exists, with no COUNT(*) that could disagree with this query.
    let mut rows = query
        .offset(page.saturating_mul(page_size))
        .limit(page_size + 1)
        .all(&db)
        .await
        .map_err(internal)?;
    let has_more = trim_overfetch(&mut rows, page_size);

    // Pre-aggregate the per-row lookups: one IN (...) GROUP BY for
    // member_count, one IN (...) for org slugs. Replaces O(2N) round-trips
    // with two constant-cost queries per page.
    let workspace_ids: Vec<Uuid> = rows.iter().map(|w| w.id).collect();
    let org_ids: Vec<Uuid> = rows.iter().filter_map(|w| w.org_id).collect();
    let member_counts = count_workspace_members_in(&db, &workspace_ids)
        .await
        .map_err(internal)?;
    let org_slugs = lookup_org_slugs_in(&db, &org_ids).await.map_err(internal)?;

    let mut out = Vec::with_capacity(rows.len());
    for w in rows {
        let org_slug = w.org_id.and_then(|id| org_slugs.get(&id).cloned());
        out.push(AdminWorkspaceRow {
            id: w.id,
            name: w.name,
            status: status_label(&w.status),
            created_at: w.created_at.to_rfc3339(),
            last_opened_at: w.last_opened_at.map(|t| t.to_rfc3339()),
            org_id: w.org_id,
            org_slug,
            member_count: member_counts.get(&w.id).copied().unwrap_or(0),
        });
    }

    // 0-indexed `page` — which is why the caller gets a URL instead of a number
    // to increment: `admin/explorer.rs` counts from 1 under the same name.
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
struct WorkspaceIdCountRow {
    workspace_id: Uuid,
    cnt: i64,
}

#[derive(FromQueryResult)]
struct OrgIdSlugRow {
    id: Uuid,
    slug: String,
}

async fn count_workspace_members_in(
    db: &sea_orm::DatabaseConnection,
    workspace_ids: &[Uuid],
) -> Result<HashMap<Uuid, i64>, sea_orm::DbErr> {
    if workspace_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = sql_placeholders(workspace_ids.len());
    let sql = format!(
        "SELECT workspace_id, COUNT(*)::bigint AS cnt FROM workspace_members \
         WHERE workspace_id IN ({placeholders}) GROUP BY workspace_id"
    );
    let values: Vec<sea_orm::Value> = workspace_ids.iter().map(|id| (*id).into()).collect();
    let rows = WorkspaceIdCountRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.workspace_id, r.cnt)).collect())
}

async fn lookup_org_slugs_in(
    db: &sea_orm::DatabaseConnection,
    org_ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, sea_orm::DbErr> {
    if org_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let placeholders = sql_placeholders(org_ids.len());
    let sql = format!("SELECT id, slug FROM organizations WHERE id IN ({placeholders})");
    let values: Vec<sea_orm::Value> = org_ids.iter().map(|id| (*id).into()).collect();
    let rows = OrgIdSlugRow::find_by_statement(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        sql,
        values,
    ))
    .all(db)
    .await?;
    Ok(rows.into_iter().map(|r| (r.id, r.slug)).collect())
}

fn sql_placeholders(n: usize) -> String {
    (1..=n)
        .map(|i| format!("${i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub async fn get_workspace_detail(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<AdminWorkspaceDetail>, StatusCode> {
    let db = establish_connection().await.map_err(internal)?;
    let ws = workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // Scope: keyed by the workspace's OWNING org, resolved above — a workspace is not
    // itself scopeable, so the fence has to read through it. See `admin::scope`.
    scope::deny_out_of_scope_opt(&db, &actor, ws.org_id).await?;

    let member_count = workspace_members::Entity::find()
        .filter(workspace_members::Column::WorkspaceId.eq(ws.id))
        .count(&db)
        .await
        .map_err(internal)? as i64;

    let (org_slug, org_name) = match ws.org_id {
        Some(org_id) => match organizations::Entity::find_by_id(org_id)
            .one(&db)
            .await
            .map_err(internal)?
        {
            Some(org) => (Some(org.slug), Some(org.name)),
            None => (None, None),
        },
        None => (None, None),
    };

    let members = load_members(&db, ws.id).await.map_err(internal)?;

    Ok(Json(AdminWorkspaceDetail {
        id: ws.id,
        name: ws.name,
        status: status_label(&ws.status),
        created_at: ws.created_at.to_rfc3339(),
        updated_at: ws.updated_at.to_rfc3339(),
        last_opened_at: ws.last_opened_at.map(|t| t.to_rfc3339()),
        org_id: ws.org_id,
        org_slug,
        org_name,
        path: ws.path,
        git_remote_url: ws.git_remote_url,
        error: ws.error,
        member_count,
        members,
        current_revision_id: ws.current_revision_id,
    }))
}

pub async fn update_workspace(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<UpdateWorkspaceBody>,
) -> Result<StatusCode, StatusCode> {
    let trimmed_name = body.name.as_ref().map(|s| s.trim().to_string());
    if trimmed_name.as_deref().is_some_and(str::is_empty) {
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = establish_connection().await.map_err(internal)?;
    let ws = workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // Scope: keyed by the workspace's OWNING org, resolved above — a workspace is not
    // itself scopeable, so the fence has to read through it. See `admin::scope`.
    scope::deny_out_of_scope_opt(&db, &actor, ws.org_id).await?;

    let mut active: workspaces::ActiveModel = ws.into();
    if let Some(name) = trimmed_name {
        active.name = Set(name);
    }
    active.updated_at = Set(Utc::now().fixed_offset());
    active.update(&db).await.map_err(internal)?;
    tracing::info!(
        admin_email = %actor.label(),
        target_id = %workspace_id,
        action = "rename_workspace",
        "admin tenant action"
    );
    Ok(StatusCode::NO_CONTENT)
}

pub async fn delete_workspace(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(internal)?;
    // Resolve the workspace BEFORE deleting it, purely so its org can be fenced — the
    // handler had no lookup at all, which is exactly why it was reachable unscoped.
    let ws = workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;
    scope::deny_out_of_scope_opt(&db, &actor, ws.org_id).await?;
    let res = workspaces::Entity::delete_by_id(workspace_id)
        .exec(&db)
        .await
        .map_err(map_destructive_db_err)?;
    if res.rows_affected == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    // Remove the deleted workspace's orphaned schedule rows (no FK cascade),
    // else its health_eval row keeps firing tasks into the dead-letter queue.
    oxy_app::server::api::workspaces::cleanup_workspace_schedules(&db, workspace_id).await;
    tracing::info!(
        admin_email = %actor.label(),
        target_id = %workspace_id,
        action = "delete_workspace",
        "admin tenant action"
    );
    Ok(StatusCode::NO_CONTENT)
}

/// Move a workspace under a different organization. Updates the workspace
/// row's `org_id` and bumps `updated_at`; **does not touch
/// `workspace_members`**.
///
/// Policy rationale (membership reconciliation is intentional non-action):
/// - `workspace_members` is keyed on (workspace_id, user_id). The FK is to
///   the workspace and the user, not to the org, so existing memberships
///   stay valid against the new org without any modification.
/// - If a member of the *old* org isn't a member of the *new* org, we
///   deliberately preserve their workspace access. Scrubbing those rows
///   would silently break setups where a workspace is being shared across
///   orgs intentionally (e.g. a consultant in the previous org continuing
///   work on the transferred workspace).
/// - Operators who *do* want to audit per-member access after a transfer
///   should pair this endpoint with the per-workspace member list
///   (`GET /admin/workspaces/{workspace_id}/detail`) and the per-user org
///   membership list (`GET /admin/users/{user_id}/org-memberships`) — both
///   already enumerate every membership row this handler is leaving
///   untouched.
pub async fn transfer_org(
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<TransferOrgBody>,
) -> Result<StatusCode, StatusCode> {
    let db = establish_connection().await.map_err(internal)?;
    // Both ends: fencing only the source would let a bounded grant move a workspace it
    // legitimately holds INTO an org it has no reach over.
    scope::deny_out_of_scope(&db, &actor, body.new_org_id).await?;
    organizations::Entity::find_by_id(body.new_org_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;

    let ws = workspaces::Entity::find_by_id(workspace_id)
        .one(&db)
        .await
        .map_err(internal)?
        .ok_or(StatusCode::NOT_FOUND)?;
    // Scope: keyed by the workspace's OWNING org, resolved above — a workspace is not
    // itself scopeable, so the fence has to read through it. See `admin::scope`.
    scope::deny_out_of_scope_opt(&db, &actor, ws.org_id).await?;

    let mut active: workspaces::ActiveModel = ws.into();
    active.org_id = Set(Some(body.new_org_id));
    active.updated_at = Set(Utc::now().fixed_offset());
    active.update(&db).await.map_err(internal)?;
    tracing::info!(
        admin_email = %actor.label(),
        target_id = %workspace_id,
        new_org_id = %body.new_org_id,
        action = "transfer_workspace_org",
        "admin tenant action"
    );
    Ok(StatusCode::NO_CONTENT)
}

async fn load_members(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Vec<WorkspaceMember>, sea_orm::DbErr> {
    let rows = workspace_members::Entity::find()
        .filter(workspace_members::Column::WorkspaceId.eq(workspace_id))
        .order_by_asc(workspace_members::Column::CreatedAt)
        .all(db)
        .await?;

    // Batch the user lookups into a single query instead of one per row.
    let user_ids: Vec<Uuid> = rows.iter().map(|m| m.user_id).collect();
    let users_by_id: HashMap<Uuid, users::Model> = users::Entity::find()
        .filter(users::Column::Id.is_in(user_ids))
        .all(db)
        .await?
        .into_iter()
        .map(|u| (u.id, u))
        .collect();

    let mut out = Vec::with_capacity(rows.len());
    for m in rows {
        if let Some(u) = users_by_id.get(&m.user_id) {
            out.push(WorkspaceMember {
                user_id: u.id,
                email: u.label().to_string(),
                name: u.name.clone(),
                role: m.role.as_str().to_string(),
                joined_at: m.created_at.to_rfc3339(),
            });
        }
    }
    Ok(out)
}

fn parse_status(input: &str) -> Result<WorkspaceStatus, StatusCode> {
    match input {
        "ready" => Ok(WorkspaceStatus::Ready),
        "cloning" => Ok(WorkspaceStatus::Cloning),
        "failed" => Ok(WorkspaceStatus::Failed),
        "not_oxy_project" => Ok(WorkspaceStatus::NotOxyProject),
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

fn status_label(status: &WorkspaceStatus) -> String {
    match status {
        WorkspaceStatus::Ready => "ready".to_string(),
        WorkspaceStatus::Cloning => "cloning".to_string(),
        WorkspaceStatus::Failed => "failed".to_string(),
        WorkspaceStatus::NotOxyProject => "not_oxy_project".to_string(),
    }
}

fn internal<E: std::fmt::Display>(e: E) -> StatusCode {
    tracing::error!("workspaces_admin internal error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

/// Map a DbErr from a destructive admin operation to a StatusCode. Only
/// foreign-key violations yield 409 — they indicate that other rows
/// (memberships, audit records, etc.) still reference this one. Connection
/// / SQL / type errors map to 500 via `internal` so we don't lie about the
/// cause to operators reading logs.
fn map_destructive_db_err(e: sea_orm::DbErr) -> StatusCode {
    use sea_orm::SqlErr;
    match e.sql_err() {
        Some(SqlErr::ForeignKeyConstraintViolation(msg)) => {
            tracing::warn!("destructive admin op blocked by FK constraint: {msg}");
            StatusCode::CONFLICT
        }
        _ => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use entity::workspace_members::WorkspaceRole;
    use std::str::FromStr;

    #[test]
    fn parse_status_accepts_known() {
        assert!(matches!(parse_status("ready"), Ok(WorkspaceStatus::Ready)));
        assert!(matches!(
            parse_status("cloning"),
            Ok(WorkspaceStatus::Cloning)
        ));
        assert!(matches!(
            parse_status("failed"),
            Ok(WorkspaceStatus::Failed)
        ));
        assert!(matches!(
            parse_status("not_oxy_project"),
            Ok(WorkspaceStatus::NotOxyProject)
        ));
    }

    #[test]
    fn parse_status_rejects_unknown_with_bad_request() {
        assert_eq!(parse_status("bogus"), Err(StatusCode::BAD_REQUEST));
    }

    #[test]
    fn workspace_role_round_trip() {
        let role = WorkspaceRole::from_str("admin").unwrap();
        assert_eq!(role.as_str(), "admin");
        assert!(matches!(role, WorkspaceRole::Admin));
    }

    #[test]
    fn update_body_rejects_only_whitespace_name() {
        let body = UpdateWorkspaceBody {
            name: Some(" ".to_string()),
        };
        let trimmed = body.name.as_ref().map(|s| s.trim().to_string());
        assert!(trimmed.as_deref().is_some_and(str::is_empty));
    }
}
