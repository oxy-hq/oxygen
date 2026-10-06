//! Read-only status for a workspace's per-org OLTP database.
//!
//! Deliberately narrower than `airhouse`'s equivalent, in two ways.
//!
//! **No credential endpoint.** Airhouse exposes `POST /me/credentials` so a user
//! can connect their own client. There is no counterpart here: a per-org OLTP
//! database holds a customer's live business records, and handing a human a
//! connection string is precisely what [`crate::schema::ANALYST_ROLE`] exists to
//! prevent. Queries go through the IDE via `type: postgres_managed`, which
//! resolves the read-only analyst server-side.
//!
//! **No provision endpoint yet.** Provisioning needs a configured provider; in
//! the POC that is the `seed_org` example. `GET /connection` reports
//! `is_provisioned: false` and the UI explains how, rather than offering a
//! button that would 503.

use axum::extract::{Json, Query};
use axum::http::StatusCode;
use entity::org_members;
use entity::prelude::{OrgMembers, Workspaces};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_platform::db::establish_connection;
use sea_orm::DatabaseConnection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use tracing::{error, instrument};
use uuid::Uuid;

use crate::entity::roles::{self as oltp_roles, Entity as OltpRoles};
use crate::entity::tenants::{self as oltp_tenants, Entity as OltpTenants, TenantStatus};
use crate::platform::PLATFORM_SCHEMA_VERSION;
use crate::schema::ANALYST_ROLE;

#[derive(Debug, Deserialize)]
pub struct WorkspaceQuery {
    pub workspace_id: Uuid,
}

/// One writer's schema, as the settings panel shows it.
#[derive(Debug, Serialize)]
pub struct SchemaInfo {
    pub schema: String,
    /// `app` or `pipeline` — the prefix follows from it (`app_` / `raw_`).
    pub kind: String,
    pub writer_name: String,
    pub role: String,
    /// Whether the read-only analyst can read this schema. `raw_*` is visible
    /// by default; `app_*` requires an explicit opt-in.
    pub analytics_visible: bool,
}

#[derive(Debug, Serialize)]
pub struct ConnectionInfoResponse {
    pub is_provisioned: bool,
    /// Populated only once provisioned; the UI gates on `is_provisioned`.
    pub host: String,
    pub database: String,
    pub provider: String,
    /// The provider's own name for this project (`oxy-org-<uuid>`), and a
    /// clickable console link when the provider has one. For Neon this is the
    /// project an operator opens to see usage or billing; `console_url` is
    /// `None` on `local`/`mock`, and the UI shows the name alone.
    pub project_name: String,
    pub console_url: Option<String>,
    pub region: String,
    pub status: String,
    /// The role every human and agent query resolves to. Read-only, always.
    pub analyst_role: String,
    /// Whether the analyst login has been minted. Without it
    /// `postgres_managed` cannot resolve.
    pub analyst_ready: bool,
    pub platform_schema_version: i32,
    pub expected_platform_schema_version: i32,
    pub schemas: Vec<SchemaInfo>,
}

impl ConnectionInfoResponse {
    fn unprovisioned() -> Self {
        Self {
            is_provisioned: false,
            host: String::new(),
            database: String::new(),
            provider: String::new(),
            project_name: String::new(),
            console_url: None,
            region: String::new(),
            status: String::new(),
            analyst_role: ANALYST_ROLE.to_string(),
            analyst_ready: false,
            platform_schema_version: 0,
            expected_platform_schema_version: PLATFORM_SCHEMA_VERSION,
            schemas: Vec::new(),
        }
    }
}

/// `GET /oltp/me/connection` — status of the caller's org OLTP database.
///
/// Returns no credentials, by design. Any org member may read this; it is
/// metadata about which schemas exist and whether analytics can see them, not
/// access to the data.
///
/// - 403 — caller is not a member of the workspace's org.
/// - 404 — workspace does not exist.
#[instrument(skip(user, query), fields(user_id = %user.id, workspace_id = %query.workspace_id))]
pub async fn get_connection(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Json<ConnectionInfoResponse>, StatusCode> {
    // "My org's database": a surface that acts as the caller. A service
    // account is refused by default (API-tokens design §3.3).
    user.refuse_service_account()?;
    let db = establish_connection().await.map_err(|e| {
        error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let org_id = resolve_caller_org(&db, query.workspace_id, &user).await?;
    Ok(Json(status_for_org(&db, org_id).await?))
}

/// Everything the UI shows about an org's OLTP database, and **no credentials**.
///
/// Shared by the member-facing settings panel and the admin console so the two
/// can never drift into disagreeing about whether a tenant is provisioned.
pub async fn status_for_org(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<ConnectionInfoResponse, StatusCode> {
    let Some(tenant) = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .one(db)
        .await
        .map_err(internal("query oltp tenant"))?
    else {
        return Ok(ConnectionInfoResponse::unprovisioned());
    };

    let roles = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
        .all(db)
        .await
        .map_err(internal("query oltp roles"))?;

    let schemas = roles
        .into_iter()
        .map(|r| {
            SchemaInfo {
                // The stored choice now, falling back to the kind's default only
                // when nobody has made one. This used to always report the default,
                // so a schema an operator had opted in or out of displayed the
                // opposite of what the database enforced — still without a live
                // connection, so a scale-to-zero database stays asleep.
                //
                // One call, no second copy of the formula: `effective_visibility`
                // reads the kind, so there is no writer name to parse and no
                // fallible arm to write a duplicate rule into.
                analytics_visible: crate::migrator::effective_visibility(
                    r.analytics_visible,
                    &r.writer_kind,
                ),
                kind: match r.writer_kind {
                    crate::entity::roles::WriterKind::App => "app".to_string(),
                    crate::entity::roles::WriterKind::Pipeline => "pipeline".to_string(),
                },
                schema: r.schema_name,
                writer_name: r.writer_name,
                role: r.role_name,
            }
        })
        .collect();

    // Computed before the struct moves the fields out of `tenant`.
    let analyst_role = crate::schema::analyst_role_for(&tenant.provider, &tenant.database_name);
    let console_url = crate::provider::console_url(&tenant.provider, &tenant.project_id);
    Ok(ConnectionInfoResponse {
        is_provisioned: tenant.status == TenantStatus::Active,
        host: tenant.host,
        database: tenant.database_name,
        provider: tenant.provider,
        project_name: tenant.project_name,
        console_url,
        region: tenant.region,
        status: tenant.status.as_str().to_string(),
        // The tenant's own name. On a shared-namespace cluster the bare
        // constant is a decoy, so the panel showed a role that does not serve
        // any query next to a DSN that names the one that does.
        analyst_role,
        analyst_ready: tenant.analyst_password_ciphertext.is_some(),
        platform_schema_version: tenant.platform_schema_version,
        expected_platform_schema_version: PLATFORM_SCHEMA_VERSION,
        schemas,
    })
}

/// Workspace → org, plus the membership check.
///
/// Membership is the whole gate here: this endpoint returns no credentials, so
/// there is nothing an elevated role would unlock.
///
/// Both `/oltp/me/*` routes name their workspace in the query and come through
/// here, so this is also where the request's API token is asked: a workspace
/// of an org that has blocked it (API-tokens design §5) answers 404, as one
/// that does not exist.
pub(super) async fn resolve_caller_org(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    user: &AuthenticatedUser,
) -> Result<Uuid, StatusCode> {
    // Local mode has a single implicit org at the nil UUID and no membership
    // rows to check. Mirrors `airhouse::api::handlers::resolve_caller_role`.
    if workspace_id.is_nil() {
        return Ok(Uuid::nil());
    }

    let workspace = Workspaces::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(internal("query workspace"))?
        .ok_or(StatusCode::NOT_FOUND)?;
    let org_id = workspace.org_id.ok_or(StatusCode::FORBIDDEN)?;
    user.require_org_reach(org_id)?;

    OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user.id))
        .one(db)
        .await
        .map_err(internal("query membership"))?
        .ok_or(StatusCode::FORBIDDEN)?;

    Ok(org_id)
}

fn internal(what: &'static str) -> impl Fn(sea_orm::DbErr) -> StatusCode {
    move |e| {
        error!("failed to {what}: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}
