//! Per-user Airhouse credential access — ephemeral-only.
//!
//! - `GET    /airhouse/me/connection`         — connection coordinates + role,
//!   no credentials. UI calls this to populate the "your warehouse" panel and
//!   to gate the "Get token" button on `is_provisioned`.
//! - `POST   /airhouse/me/credentials`        — mint a fresh ephemeral via
//!   the broker. POST because each call writes an audit row on the airhouse
//!   side and consumes mint quota. Each call produces a new (`username`,
//!   `password`) pair with an `expires_at`. **There is no stored password.**
//!   Subsequent calls return new credentials; old ones expire on their own
//!   (or via the revoke endpoint below).
//! - `POST   /airhouse/me/provision`          — ensure the per-org tenant +
//!   per-tenant service account exist. Idempotent. Body: `{ "tenant_name":
//!   "<name>" }` (required on first provision). No per-user provisioning —
//!   the SA is what mints later.
//! - `DELETE /airhouse/me/tokens/{username}`  — revoke a single ephemeral
//!   credential the caller previously minted. The username is the opaque
//!   `eph_*` returned by `/credentials`.

use axum::extract::{Json, Path, Query};
use axum::http::StatusCode;
use entity::org_members;
use entity::prelude::{OrgMembers, Workspaces};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_platform::db::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::{Deserialize, Serialize};
use tracing::{error, instrument, warn};
use uuid::Uuid;

use crate::admin::CatalogIndexesResponse;
use crate::airhouse_role_for;
use crate::broker::DEFAULT_EXTERNAL_TTL;
use crate::config::{admin_client, provisioner_for, token_broker, wire_endpoint};
use crate::entity::Tenants as AirhouseTenants;
use crate::entity::tenants::{self as airhouse_tenants, TenantStatus};
use crate::provisioner::ProvisionerError;

/// Bridges the entity-level `OrgRole` to the workspace-level `WorkspaceRole`
/// the broker's `airhouse_role_for` mapping accepts. Mirrors the trivial
/// 1-to-1 lift used elsewhere; broken out here so the handler doesn't have
/// to import entity::workspace_members.
fn workspace_role_from_org_role(
    role: &org_members::OrgRole,
) -> entity::workspace_members::WorkspaceRole {
    match role {
        org_members::OrgRole::Owner => entity::workspace_members::WorkspaceRole::Owner,
        org_members::OrgRole::Admin => entity::workspace_members::WorkspaceRole::Admin,
        org_members::OrgRole::Member => entity::workspace_members::WorkspaceRole::Member,
    }
}

/// Compose the effective workspace role from `org_role` and any
/// `workspace_members` override, mirroring
/// `app::server::api::middlewares::workspace_context::resolve_workspace_membership`
/// — overrides can only elevate, never downgrade. Kept here so the airhouse
/// routes don't accidentally diverge from the rest of the app's role
/// resolution (a member with a workspace-level Admin override would get
/// Reader through the IDE token panel but Writer through agent runs
/// otherwise).
async fn lookup_effective_workspace_role(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    user_id: Uuid,
    org_role: &org_members::OrgRole,
) -> Result<entity::workspace_members::WorkspaceRole, StatusCode> {
    use entity::prelude::WorkspaceMembers;
    use entity::workspace_members::Column as WsMemberCol;

    let org_derived = workspace_role_from_org_role(org_role);
    let override_row = WorkspaceMembers::find()
        .filter(WsMemberCol::WorkspaceId.eq(workspace_id))
        .filter(WsMemberCol::UserId.eq(user_id))
        .one(db)
        .await
        .map_err(|e| {
            error!("Failed to query workspace member override: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    Ok(match override_row {
        Some(member) => std::cmp::max(org_derived, member.role),
        None => org_derived,
    })
}

#[derive(Deserialize)]
pub struct WorkspaceQuery {
    pub workspace_id: Uuid,
}

#[derive(Deserialize)]
pub struct ProvisionBody {
    pub tenant_name: String,
}

#[derive(Serialize)]
pub struct ConnectionInfoResponse {
    pub host: String,
    pub port: u16,
    pub dbname: String,
    /// The Airhouse role the caller's mints will carry. Not the underlying
    /// `org_members.role` — that's mapped (`Owner→admin`, `Admin→writer`,
    /// `Member/Viewer→reader`) to the airhouse role string before being
    /// sent down.
    pub role: String,
    /// `true` once both the tenant and its service account exist on the
    /// airhouse side. Until this flips the UI should hide the "get token"
    /// affordance and show the "provision" CTA.
    pub is_provisioned: bool,
}

#[derive(Serialize)]
pub struct EphemeralTokenResponse {
    pub username: String,
    /// Plaintext credential. Returned exactly once on this response — the
    /// server does not persist it. Lost passwords are lost; mint a new one.
    pub password: String,
    pub host: String,
    pub port: u16,
    pub dbname: String,
    pub role: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
}

/// Resolve `(org_id, oxy role)` for the caller in the workspace's org. The
/// local nil-UUID workspace bypasses the DB check — there's a seeded local
/// org and the local guest is unconditionally Owner.
///
/// Every `/airhouse/me/*` route names its workspace in the query and comes
/// through here, so this is also where the request's API token is asked: a
/// workspace of an org that has blocked it (API-tokens design §5) answers 404,
/// as one that does not exist.
async fn resolve_caller_role(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
    user: &AuthenticatedUser,
) -> Result<(Uuid, org_members::OrgRole), StatusCode> {
    if workspace_id.is_nil() {
        return Ok((Uuid::nil(), org_members::OrgRole::Owner));
    }
    let workspace = Workspaces::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(|e| {
            error!("Failed to query workspace: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;
    let org_id = workspace.org_id.ok_or(StatusCode::FORBIDDEN)?;
    user.require_org_reach(org_id)?;

    let membership = OrgMembers::find()
        .filter(org_members::Column::OrgId.eq(org_id))
        .filter(org_members::Column::UserId.eq(user.id))
        .one(db)
        .await
        .map_err(|e| {
            error!("Failed to query membership: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::FORBIDDEN)?;
    Ok((org_id, membership.role.clone()))
}

/// Lookup the local airhouse_tenants row, returning the tenant id string
/// when the row exists with both SA fields populated. `None` means "not
/// provisioned (yet, or rolled back to failed)".
async fn lookup_provisioned_tenant(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Option<String>, StatusCode> {
    let row = AirhouseTenants::find()
        .filter(airhouse_tenants::Column::WorkspaceId.eq(workspace_id))
        .one(db)
        .await
        .map_err(|e| {
            error!("Failed to fetch airhouse_tenants row: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    let Some(row) = row else { return Ok(None) };
    if row.status != TenantStatus::Active {
        return Ok(None);
    }
    if row.service_account_id.is_none() || row.bearer_ciphertext.is_none() {
        return Ok(None);
    }
    Ok(Some(row.airhouse_tenant_id))
}

#[derive(Serialize)]
pub struct VersionResponse {
    /// The Airhouse server's software version (its `CARGO_PKG_VERSION`),
    /// read live from the deployment's public `/health` endpoint.
    pub version: String,
}

/// `GET /airhouse/version` — the running Airhouse deployment's software
/// version, surfaced next to the connection panel like Oxy's own
/// VersionBadge. Global (one Airhouse per deployment), so unlike the
/// `/me/*` routes it takes no `workspace_id` and resolves no role.
///
/// - 503 when Airhouse is disabled/misconfigured at the deployment level
///   (no env vars) — same signal `get_connection` uses.
/// - 502 when Airhouse's `/health` is unreachable or returns an unexpected
///   shape. The UI treats either non-200 as "hide the badge".
#[instrument(skip_all)]
pub async fn get_version(
    AuthenticatedUserExtractor(_user): AuthenticatedUserExtractor,
) -> Result<Json<VersionResponse>, StatusCode> {
    let client = admin_client().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let version = client.server_version().await.map_err(|e| {
        warn!("failed to fetch airhouse server version: {e}");
        StatusCode::BAD_GATEWAY
    })?;
    Ok(Json(VersionResponse { version }))
}

/// `GET /airhouse/me/connection` — return wire endpoint, dbname, and the
/// caller's effective airhouse role; flag `is_provisioned` so the UI knows
/// whether the "get token" button is meaningful yet.
///
/// - 503 when Airhouse is disabled at the deployment level (no env vars).
/// - 403 when the caller is not a member of the org owning this workspace.
/// - 200 with `is_provisioned: false` when the tenant has not been
///   provisioned yet — the UI should show the "Provision Airhouse access"
///   CTA. Once provisioned, the same shape returns with `is_provisioned:
///   true`; tokens are minted on-demand from `/credentials`.
#[instrument(skip(user, query), fields(user_id = %user.id, workspace_id = %query.workspace_id))]
pub async fn get_connection(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Json<ConnectionInfoResponse>, StatusCode> {
    let endpoint = wire_endpoint().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let db = establish_connection().await.map_err(|e| {
        error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (_org_id, oxy_role) = resolve_caller_role(&db, query.workspace_id, &user).await?;
    let workspace_role =
        lookup_effective_workspace_role(&db, query.workspace_id, user.id, &oxy_role).await?;
    let role = airhouse_role_for(workspace_role);

    let dbname = lookup_provisioned_tenant(&db, query.workspace_id).await?;
    let is_provisioned = dbname.is_some();

    Ok(Json(ConnectionInfoResponse {
        host: endpoint.host,
        port: endpoint.port,
        // Until the tenant is provisioned the dbname doesn't exist yet —
        // surface an empty string rather than a placeholder; the UI gates
        // on `is_provisioned` anyway.
        dbname: dbname.unwrap_or_default(),
        role: role.as_str().to_string(),
        is_provisioned,
    }))
}

/// `POST /airhouse/me/credentials` — mint a fresh ephemeral credential via
/// the broker. POST because each call writes an audit row on airhouse and
/// consumes mint quota; GET would violate HTTP's safe/idempotent contract.
/// The response is the only place this credential is ever seen by the
/// caller; the server does not persist it. Each call mints a new pair —
/// there is no "rotate" or "reveal stored". Old credentials either expire
/// naturally or can be revoked via `DELETE /airhouse/me/tokens/{username}`.
///
/// - 503 — airhouse disabled at the deployment level.
/// - 403 — caller is not an org member.
/// - 409 — tenant has not been provisioned yet (UI should redirect to
///   `/provision`).
/// - 502 — airhouse mint endpoint rejected the request (rate-limited,
///   SA revoked, etc.).
#[instrument(skip(user, query), fields(user_id = %user.id, workspace_id = %query.workspace_id))]
pub async fn get_credentials(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Json<EphemeralTokenResponse>, StatusCode> {
    // The credential minted here is the caller's own, at the caller's role. A
    // service account has no person to hold it: refused by default.
    user.refuse_service_account()?;
    let endpoint = wire_endpoint().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let db = establish_connection().await.map_err(|e| {
        error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (_org_id, oxy_role) = resolve_caller_role(&db, query.workspace_id, &user).await?;
    let workspace_role =
        lookup_effective_workspace_role(&db, query.workspace_id, user.id, &oxy_role).await?;
    let airhouse_role = airhouse_role_for(workspace_role);

    let _tenant_id = lookup_provisioned_tenant(&db, query.workspace_id)
        .await?
        .ok_or(StatusCode::CONFLICT)?;

    let broker = token_broker().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let cred = broker
        .mint_for_user(
            query.workspace_id,
            user.id,
            airhouse_role,
            DEFAULT_EXTERNAL_TTL,
        )
        .await
        .map_err(|e| {
            warn!(workspace_id = %query.workspace_id, "credential mint failed: {e}");
            StatusCode::BAD_GATEWAY
        })?;

    Ok(Json(EphemeralTokenResponse {
        username: cred.username,
        password: cred.password,
        host: endpoint.host,
        port: endpoint.port,
        dbname: cred.tenant,
        role: cred.role,
        expires_at: cred.expires_at,
    }))
}

/// `POST /airhouse/me/provision` — ensure the workspace's tenant + SA
/// exist on the airhouse side. Idempotent; safe to retry. Returns the
/// same shape as `GET /connection` so the UI can swap out the local
/// state in one round-trip.
///
/// - 503 — airhouse disabled.
/// - 403 — caller is not an org Owner/Admin of the workspace's org.
///   Provisioning consumes a global tenant-name and creates a service
///   account, so plain Members are not allowed to claim a name on
///   behalf of the org.
/// - 422 — `tenant_name` failed validation (lowercase alnum/-/_, ≤63 chars,
///   leading letter).
/// - 502 — airhouse rejected tenant or SA creation.
#[instrument(skip(user, query, body), fields(user_id = %user.id, workspace_id = %query.workspace_id))]
pub async fn provision(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(query): Query<WorkspaceQuery>,
    Json(body): Json<ProvisionBody>,
) -> Result<Json<ConnectionInfoResponse>, StatusCode> {
    user.refuse_service_account()?;
    let endpoint = wire_endpoint().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let db = establish_connection().await.map_err(|e| {
        error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let (_org_id, oxy_role) = resolve_caller_role(&db, query.workspace_id, &user).await?;

    // Provisioning consumes a global tenant-name and mints an SA bearer.
    // Treat it like other org-scoped admin actions and reject plain
    // Members at the wire — the UI also gates the button, but the
    // endpoint must enforce on its own.
    if !matches!(
        oxy_role,
        org_members::OrgRole::Owner | org_members::OrgRole::Admin
    ) {
        warn!(
            workspace_id = %query.workspace_id,
            user_id = %user.id,
            role = ?oxy_role,
            "rejecting non-admin attempt to provision airhouse tenant"
        );
        return Err(StatusCode::FORBIDDEN);
    }

    let workspace_role =
        lookup_effective_workspace_role(&db, query.workspace_id, user.id, &oxy_role).await?;
    let airhouse_role = airhouse_role_for(workspace_role);

    let tenant_prov = provisioner_for(db.clone()).ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let tenant = tenant_prov
        .provision(query.workspace_id, body.tenant_name)
        .await
        .map_err(|e| match e {
            ProvisionerError::InvalidTenantName(_) => StatusCode::UNPROCESSABLE_ENTITY,
            // Surface name collisions as a real 409 so the UI can ask the
            // user for a different name without conflating it with
            // upstream unavailability (which is 502 below).
            ProvisionerError::TenantNameTaken(_) => StatusCode::CONFLICT,
            other => {
                warn!(
                    workspace_id = %query.workspace_id,
                    "tenant provisioning failed: {other}"
                );
                StatusCode::BAD_GATEWAY
            }
        })?;

    Ok(Json(ConnectionInfoResponse {
        host: endpoint.host,
        port: endpoint.port,
        dbname: tenant.id,
        role: airhouse_role.as_str().to_string(),
        is_provisioned: true,
    }))
}

#[derive(Deserialize)]
pub struct SetCatalogIndexesBody {
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct SetCatalogIndexesResponse {
    /// Whether airhouse accepted the async apply (its HTTP 202).
    pub accepted: bool,
}

/// `GET /airhouse/me/catalog-indexes?workspace_id=…` — presence + validity of
/// the workspace tenant's DuckLake catalog hot-path indexes. Owner/Admin only;
/// read-through to the airhouse admin API. `404` when the workspace has no
/// provisioned tenant.
#[instrument(skip(user, query), fields(user_id = %user.id, workspace_id = %query.workspace_id))]
pub async fn get_catalog_indexes(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(query): Query<WorkspaceQuery>,
) -> Result<Json<CatalogIndexesResponse>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let (_org_id, oxy_role) = resolve_caller_role(&db, query.workspace_id, &user).await?;
    require_workspace_admin(oxy_role, query.workspace_id, user.id)?;

    let tenant_id = lookup_provisioned_tenant(&db, query.workspace_id)
        .await?
        .ok_or(StatusCode::NOT_FOUND)?;
    let client = admin_client().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let status = client.catalog_index_status(&tenant_id).await.map_err(|e| {
        warn!(workspace_id = %query.workspace_id, "catalog index status failed: {e}");
        StatusCode::BAD_GATEWAY
    })?;
    Ok(Json(status))
}

/// `PUT /airhouse/me/catalog-indexes?workspace_id=…` — toggle the workspace
/// tenant's DuckLake catalog hot-path indexes on/off. Owner/Admin only. The
/// apply runs asynchronously on airhouse (`CREATE INDEX CONCURRENTLY`);
/// `accepted: true` means it was accepted — poll `GET` until `state == "ready"`.
#[instrument(skip(user, query, body), fields(user_id = %user.id, workspace_id = %query.workspace_id))]
pub async fn set_catalog_indexes(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(query): Query<WorkspaceQuery>,
    Json(body): Json<SetCatalogIndexesBody>,
) -> Result<Json<SetCatalogIndexesResponse>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let (_org_id, oxy_role) = resolve_caller_role(&db, query.workspace_id, &user).await?;
    require_workspace_admin(oxy_role, query.workspace_id, user.id)?;

    let tenant_id = lookup_provisioned_tenant(&db, query.workspace_id)
        .await?
        .ok_or(StatusCode::NOT_FOUND)?;
    let client = admin_client().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let accepted = client
        .set_catalog_indexes(&tenant_id, body.enabled)
        .await
        .map_err(|e| {
            warn!(workspace_id = %query.workspace_id, "catalog index toggle failed: {e}");
            StatusCode::BAD_GATEWAY
        })?;
    Ok(Json(SetCatalogIndexesResponse { accepted }))
}

/// Reject non-owner/admin callers at the wire (the UI also gates the toggle,
/// but the endpoint must enforce independently). Mirrors the inline check in
/// [`provision`].
fn require_workspace_admin(
    oxy_role: org_members::OrgRole,
    workspace_id: Uuid,
    user_id: Uuid,
) -> Result<(), StatusCode> {
    if !matches!(
        oxy_role,
        org_members::OrgRole::Owner | org_members::OrgRole::Admin
    ) {
        warn!(
            %workspace_id, %user_id, role = ?oxy_role,
            "rejecting non-admin catalog-index request"
        );
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

/// `DELETE /airhouse/me/tokens/{username}` — revoke a single ephemeral the
/// caller previously minted. Idempotent: 204 on a fresh revoke OR on an
/// already-gone username.
///
/// Authentication ties revocation to the per-tenant SA on the airhouse
/// side, so a caller can only revoke ephemerals minted under their own
/// workspace (cross-tenant attempts get a 403 from airhouse, surfaced as
/// 502 here).
/// Mint usernames are always `eph_` followed by lowercase hex
/// (`/eph_a1b2c3d4ef98`). Reject anything else before forwarding to
/// airhouse: the username is interpolated into the upstream URL via
/// `format!`, so a value like `acme/tokens/eph_target` would re-target the
/// admin client at a different path. URL-encoding would also work but a
/// strict allow-list better reflects what the broker can actually mint.
///
/// Uses `strip_prefix` rather than `s[4..]` so the suffix split survives
/// any future prefix change without risk of slicing through a multi-byte
/// UTF-8 boundary.
fn is_valid_ephemeral_username(s: &str) -> bool {
    s.len() <= 64
        && s.strip_prefix("eph_")
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit()))
}

#[instrument(skip(user, query), fields(user_id = %user.id, workspace_id = %query.workspace_id, username = %username))]
pub async fn revoke_token(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(query): Query<WorkspaceQuery>,
    Path(username): Path<String>,
) -> Result<StatusCode, StatusCode> {
    if !is_valid_ephemeral_username(&username) {
        warn!(
            workspace_id = %query.workspace_id,
            username = %username,
            "rejecting revoke for malformed ephemeral username"
        );
        return Err(StatusCode::BAD_REQUEST);
    }

    let db = establish_connection().await.map_err(|e| {
        error!("DB connection error: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let (_org_id, _role) = resolve_caller_role(&db, query.workspace_id, &user).await?;

    let broker = token_broker().ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    broker
        .revoke_user_token(query.workspace_id, &username)
        .await
        .map_err(|e| {
            warn!(workspace_id = %query.workspace_id, "token revocation failed: {e}");
            StatusCode::BAD_GATEWAY
        })?;
    Ok(StatusCode::NO_CONTENT)
}
