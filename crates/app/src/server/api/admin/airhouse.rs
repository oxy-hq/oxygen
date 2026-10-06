//! Scope fence for the admin Airhouse surface.
//!
//! A fenced shim: the handlers live in the
//! `airhouse` crate, which sits below `oxy-app` and cannot read `app_admins`,
//! so the routes are declared here and each one is fenced before delegating.
//! `cap(..)` decides on `Resource::platform()` — a nil org — so on its own it
//! lets a `global_admin` bounded to two orgs act on every workspace on the
//! deployment.
//!
//! **`deny_out_of_scope_opt`, not `deny_out_of_scope`.** An Airhouse tenant is
//! keyed by workspace, and a workspace's `org_id` is nullable. `if let
//! Some(org) = ws.org_id { deny(..) }` is the obvious spelling and the wrong
//! one — the `None` arm is not a check that passes, it is no check at all. The
//! `_opt` helper refuses a null org for a bounded grant, which is the answer
//! this console gives everywhere else.

use axum::Router;
use axum::extract::{Json, Path};
use axum::http::StatusCode;
use axum::routing::{get, post};
use oxy::database::client::establish_connection;
use oxy_app_core::audit;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use uuid::Uuid;

use airhouse::api::admin as inner;

use super::scope;

pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    // `admin::router()` supplies the `/admin` prefix at the mount.
    Router::new().route("/airhouse", get(list_fleet)).route(
        "/workspaces/{workspace_id}/airhouse/provision",
        post(provision),
    )
}

async fn conn() -> Result<sea_orm::DatabaseConnection, StatusCode> {
    establish_connection().await.map_err(|e| {
        tracing::error!("airhouse admin: connect: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// The workspace's org, for the fence. A workspace that does not exist is a
/// 404 here rather than inside the handler, so the fence never runs against a
/// `None` it cannot interpret.
async fn workspace_org(
    db: &sea_orm::DatabaseConnection,
    workspace_id: Uuid,
) -> Result<Option<Uuid>, StatusCode> {
    use sea_orm::EntityTrait;
    entity::prelude::Workspaces::find_by_id(workspace_id)
        .one(db)
        .await
        .map_err(|e| {
            tracing::error!("airhouse admin: load workspace: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .map(|w| w.org_id)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Filtered, not refused: a bounded operator sees their workspaces rather than
/// an error. The lenient read-path answer, matching `apps::handlers`.
///
/// The scope goes INTO the query rather than being applied to its result — a
/// bounded operator was paying for every workspace on the deployment and then
/// throwing most of them away.
pub async fn list_fleet(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<inner::AirhouseFleet>, StatusCode> {
    let db = conn().await?;
    let scope = super::apps::handlers::scope_org_filter(&db, &user).await;
    inner::list_fleet(&user, scope.as_deref()).await
}

/// Audited, like every other staff mutation on this console.
///
/// This was the one that was not. Provisioning a warehouse
/// for a tenant you are acting on behalf of consumes a global tenant name
/// permanently and creates an external resource, from one unconfirmed click.
/// `/admin/audit` is where an operator goes to find out who did it.
pub async fn provision(
    user: oxy_app_core::audit::RequestActor,
    Path(workspace_id): Path<Uuid>,
) -> Result<Json<inner::AirhouseFleetRow>, (StatusCode, String)> {
    let db = conn().await.map_err(msg)?;
    let org_id = workspace_org(&db, workspace_id).await.map_err(msg)?;
    scope::deny_out_of_scope_opt(&db, &user, org_id)
        .await
        .map_err(msg)?;

    let out = inner::provision(
        AuthenticatedUserExtractor(user.user.clone()),
        Path(workspace_id),
    )
    .await;
    // Best-effort: an audit write must never turn a successful provision into a
    // 500, and a failure is already logged at `error` by `record_best_effort`.
    let entry = audit::AuditEntry::for_request(&user, "airhouse.provisioned")
        .workspace(workspace_id)
        .target(
            "airhouse_tenant",
            workspace_id.to_string(),
            out.as_ref()
                .map(|Json(r)| r.tenant_id.clone())
                .unwrap_or_default(),
        )
        // The name this call ASKED for, recorded always — on the failure path
        // the target label is empty, and a `TenantNameTaken` is precisely when
        // an operator needs to know which name was attempted.
        //
        // Named `requested_`, not `tenant_name`: provisioning is idempotent, so
        // a workspace that already had a tenant comes back with whatever name
        // it was created under, which need not be the derived one. The name
        // that ended up in force is the target label above.
        .metadata(serde_json::json!({
            "requested_tenant_name": airhouse::api::admin::tenant_name_for_workspace(workspace_id),
        }));
    let entry = match org_id {
        Some(org) => entry.org(org),
        None => entry,
    };
    // The reason the provision failed, not the fact that it did. "handler
    // returned an error" is the same string for a name collision, a 503 from an
    // unconfigured deployment, and a warehouse-side outage — three things an
    // operator reading `/admin/audit` needs to tell apart, and the audit row is
    // where they look precisely because the response is long gone.
    let entry = match out.as_ref() {
        Ok(_) => entry,
        Err((status, why)) => entry.failure(format!("{status}: {why}")),
    };
    audit::record_best_effort(&db, entry).await;

    out
}

/// 404 is the fence's deliberate answer — an operator with no reach must not
/// learn the workspace exists — so the message must not contradict the status.
///
/// Paired per status rather than a blanket "not found": a `DbErr` inside
/// `workspace_org` is a 500, and calling that "not found" turns an outage into
/// a missing workspace.
fn msg(s: StatusCode) -> (StatusCode, String) {
    let body = match s {
        StatusCode::NOT_FOUND => "not found",
        StatusCode::INTERNAL_SERVER_ERROR => "internal error",
        _ => "request rejected",
    };
    (s, body.to_string())
}
