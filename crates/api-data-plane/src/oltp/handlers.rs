//! Read-only status for a workspace's per-org OLTP database.
//!
//! Deliberately narrower than `airhouse`'s equivalent, in two ways.
//!
//! **No credential endpoint.** Airhouse exposes `POST /me/credentials` so a user
//! can connect their own client. There is no counterpart here: a per-org OLTP
//! database holds a customer's live business records, and handing a human a
//! connection string is precisely what [`oxy_oltp::schema::ANALYST_ROLE`] exists to
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

use oxy_oltp::config::DEFAULT_PG_VERSION;
use oxy_oltp::entity::roles::{self as oltp_roles, Entity as OltpRoles};
use oxy_oltp::entity::tenants::{self as oltp_tenants, Entity as OltpTenants, TenantStatus};
use oxy_oltp::platform::PLATFORM_SCHEMA_VERSION;
use oxy_oltp::schema::ANALYST_ROLE;

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
    /// The tenant's Postgres major, as `oltp_tenants.pg_version` records it
    /// (provisioning, then every reconcile writes the provider's live answer).
    /// `null` when the org has no database. Read from the stored row: no
    /// provider call, so a scale-to-zero database stays asleep.
    pub pg_version: Option<i16>,
    /// The major this deployment provisions a new database on:
    /// `OXY_OLTP_PG_VERSION` when set, else `DEFAULT_PG_VERSION`.
    pub default_pg_version: u8,
    /// True when a MANAGED tenant's major differs from `default_pg_version`.
    /// See [`pg_version_drift`].
    pub pg_version_drift: bool,
    pub schemas: Vec<SchemaInfo>,
}

/// Whether a tenant's Postgres major is a real gap an operator can act on.
///
/// Restored from the deleted Rust `oxy oltp status`: `oltp_tenants.pg_version`
/// was written by provisioning and read by nothing, so a tenant left on an
/// older major than the one Oxy now provisions was invisible everywhere.
///
/// - **Managed providers only.** There the default is what Oxy requests, so a
///   row below it names a real action. On `local` the row holds the cluster's
///   OWN major and the requested version is not honoured at all, so a developer
///   on 17 would get a permanent note pointing at something they cannot do.
/// - **`!=`, not `<`.** It is also true for a tenant AHEAD of the default; the
///   name claims a difference, not a direction.
pub fn pg_version_drift(provider: &str, pg_version: i16, default: u8) -> bool {
    provider != "local" && u8::try_from(pg_version).ok() != Some(default)
}

/// The major provisioning requests: the deployment's runtime config (which
/// honours `OXY_OLTP_PG_VERSION`), or the constant when OLTP is not configured.
/// The deleted command compared to the constant because it ran in the
/// operator's process; the server holds the real config.
fn provisioning_pg_version() -> u8 {
    oxy_oltp::config::OltpConfig::cached()
        .as_runtime()
        .map(|c| c.pg_version)
        .unwrap_or(DEFAULT_PG_VERSION)
}

impl ConnectionInfoResponse {
    fn unprovisioned(default_pg_version: u8) -> Self {
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
            pg_version: None,
            default_pg_version,
            pg_version_drift: false,
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
        return Ok(ConnectionInfoResponse::unprovisioned(
            provisioning_pg_version(),
        ));
    };

    let roles = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant.id))
        .all(db)
        .await
        .map_err(internal("query oltp roles"))?;

    Ok(response_for(tenant, roles, provisioning_pg_version()))
}

/// A tenant row and its roles → the document. Pure, so the shape is pinned
/// without a database.
fn response_for(
    tenant: oltp_tenants::Model,
    roles: Vec<oltp_roles::Model>,
    default_pg_version: u8,
) -> ConnectionInfoResponse {
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
                analytics_visible: oxy_oltp::migrator::effective_visibility(
                    r.analytics_visible,
                    &r.writer_kind,
                ),
                kind: match r.writer_kind {
                    oxy_oltp::entity::roles::WriterKind::App => "app".to_string(),
                    oxy_oltp::entity::roles::WriterKind::Pipeline => "pipeline".to_string(),
                },
                schema: r.schema_name,
                writer_name: r.writer_name,
                role: r.role_name,
            }
        })
        .collect();

    // Computed before the struct moves the fields out of `tenant`.
    let analyst_role = oxy_oltp::schema::analyst_role_for(&tenant.provider, &tenant.database_name);
    let console_url = oxy_oltp::provider::console_url(&tenant.provider, &tenant.project_id);
    let pg_version_drift =
        pg_version_drift(&tenant.provider, tenant.pg_version, default_pg_version);
    ConnectionInfoResponse {
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
        pg_version: Some(tenant.pg_version),
        default_pg_version,
        pg_version_drift,
        schemas,
    }
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
    // rows to check. Mirrors `oxy_api_data_plane::airhouse::handlers::resolve_caller_role`.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The local cluster's own major is never drift; any managed difference is.
    #[test]
    fn pg_version_drift_is_managed_only_and_either_direction() {
        let d = i16::from(DEFAULT_PG_VERSION);
        assert!(!pg_version_drift("neon", d, DEFAULT_PG_VERSION));
        assert!(pg_version_drift("neon", d - 1, DEFAULT_PG_VERSION));
        assert!(
            pg_version_drift("neon", d + 1, DEFAULT_PG_VERSION),
            "ahead of the default differs too"
        );
        assert!(pg_version_drift("mock", d - 1, DEFAULT_PG_VERSION));
        assert!(
            !pg_version_drift("local", d - 1, DEFAULT_PG_VERSION),
            "local reports its cluster's own major"
        );
        assert!(
            pg_version_drift("neon", -1, DEFAULT_PG_VERSION),
            "an out-of-range row is not the default"
        );
    }

    /// A tenant row, as the control plane stores it.
    fn tenant(provider: &str, pg_version: i16) -> oltp_tenants::Model {
        serde_json::from_value(serde_json::json!({
            "id": Uuid::nil(),
            "org_id": Uuid::nil(),
            "provider": provider,
            "project_id": "p",
            "branch_id": "b",
            "project_name": "oxy-org-x",
            "region": "aws-us-east-2",
            "pg_version": pg_version,
            "host": "h",
            "database_name": "d",
            "owner_role": "o",
            "owner_password_ciphertext": null,
            "analyst_password_ciphertext": null,
            "status": "active",
            "platform_schema_version": PLATFORM_SCHEMA_VERSION,
            "created_at": "2026-11-21T00:00:00Z",
            "updated_at": "2026-11-21T00:00:00Z"
        }))
        .unwrap()
    }

    /// The stored row's major reaches the document, with drift on a managed
    /// tenant and never on `local`.
    #[test]
    fn provisioned_document_reports_the_stored_major() {
        let v = serde_json::to_value(response_for(tenant("neon", 17), vec![], 18)).unwrap();
        assert_eq!(v["pg_version"], serde_json::json!(17));
        assert_eq!(
            v["default_pg_version"],
            serde_json::json!(DEFAULT_PG_VERSION)
        );
        assert_eq!(v["pg_version_drift"], serde_json::json!(true));

        let v = serde_json::to_value(response_for(tenant("local", 17), vec![], 18)).unwrap();
        assert_eq!(v["pg_version"], serde_json::json!(17));
        assert_eq!(v["pg_version_drift"], serde_json::json!(false));
    }

    /// `oxyc oltp status --json` parses these keys.
    #[test]
    fn unprovisioned_document_carries_the_version_fields() {
        let v = serde_json::to_value(ConnectionInfoResponse::unprovisioned(DEFAULT_PG_VERSION))
            .unwrap();
        assert_eq!(v["pg_version"], serde_json::Value::Null);
        assert_eq!(
            v["default_pg_version"],
            serde_json::json!(DEFAULT_PG_VERSION)
        );
        assert_eq!(v["pg_version_drift"], serde_json::json!(false));
    }

    /// A deployment that sets `OXY_OLTP_PG_VERSION=17` provisions on 17, so a
    /// managed tenant on 17 is not drift there, and one on 18 is.
    #[test]
    fn drift_follows_the_deployment_major_not_the_constant() {
        let v = serde_json::to_value(response_for(tenant("neon", 17), vec![], 17)).unwrap();
        assert_eq!(v["default_pg_version"], serde_json::json!(17));
        assert_eq!(v["pg_version_drift"], serde_json::json!(false));
        let v = serde_json::to_value(response_for(tenant("neon", 18), vec![], 17)).unwrap();
        assert_eq!(v["pg_version_drift"], serde_json::json!(true));
    }
}
