//! Scope fence + audit for the admin OLTP surface.
//!
//! The handlers themselves live in `oxy_oltp::api::admin`. This module owns the
//! *routes*, so every org-keyed one passes through [`scope::deny_out_of_scope`]
//! before the handler runs.
//!
//! **Why a shim and not a check inside `oxy-oltp`.** The fence reads
//! `app_admins` through `server::authz::globals`, which lives in `oxy-app`;
//! `oxy-oltp` sits below it and cannot import it without a cycle. Every sibling
//! console surface solves it the same way — `orgs_admin.rs`, `users_admin.rs`
//! and `apps::handlers` all call the fence in the handler — so this is the
//! established shape rather than a new one.
//!
//! **Why it matters more here than it looks.** `platform_cap_guard` decides on
//! `Resource::platform()`, whose org is nil, so a `global_admin` **bounded to
//! two orgs** passes `Action::PlatformOltp` for every org on the deployment.
//! Without this module that reached `POST …/credentials` (a write DSN for any
//! tenant) and then `DELETE …/oltp`, which calls `provider.delete_project` —
//! on Neon, the destruction of a database and every business record in it, for
//! an org the caller cannot otherwise see. `scope.rs`'s own header counts three
//! prior occurrences of exactly this omission.
//!
//! The fleet list is filtered rather than refused: a bounded operator sees
//! their orgs, not an error.

use axum::Router;
use axum::extract::{Json, Path};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use oxy::database::client::establish_connection;
use oxy_app_core::audit;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_oltp::api::admin as inner;
use uuid::Uuid;

use super::apps::handlers::scope_org_filter;
use super::scope;

pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    // Same paths as before; `admin::router()` supplies the `/admin` prefix.
    Router::new()
        .route("/oltp", get(list_tenants))
        .route("/orgs/{org_id}/oltp", get(get_status))
        .route("/orgs/{org_id}/oltp/provision", post(provision))
        .route("/orgs/{org_id}/oltp/credentials", post(credentials))
        .route("/orgs/{org_id}/oltp/visibility", post(set_visibility))
        // Releasing ONE app's store. A distinct PATH from the whole-tenant
        // delete below, not a different verb on the same one: these two differ
        // by the entire tenant, and a URL where a typo'd method destroys a
        // customer's database is a URL nobody should have to be careful around.
        .route(
            "/orgs/{org_id}/oltp/deprovision-writer",
            post(deprovision_writer),
        )
        .route("/orgs/{org_id}/oltp", delete(deprovision))
        // The org's staging branch. Created through `…/provision` with
        // `"branch": "staging"`; these read it and reset it. A reset is a POST
        // that does nothing without `"confirm": true` (428 + the apps it
        // would hit), because it discards every app's staging data at once.
        .route("/orgs/{org_id}/oltp/branches/{branch}", get(get_branch))
        .route(
            "/orgs/{org_id}/oltp/branches/{branch}/reset",
            post(reset_branch),
        )
}

/// A connection, or a 500 that says which surface failed.
///
/// Deliberately NOT "connect and fence": every handler below spells
/// `scope::deny_out_of_scope` out itself. `app_scope_boundary.rs` reads handler
/// bodies for that exact call, so a helper that hid it would pass the test
/// while making the property it pins invisible — and the fence is a rule that
/// has now been forgotten four times.
async fn conn() -> Result<sea_orm::DatabaseConnection, StatusCode> {
    establish_connection().await.map_err(|e| {
        tracing::error!("oltp admin: connect: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })
}

/// 404 is the fence's deliberate answer — an operator with no reach must not
/// learn the org exists — so the message must not contradict the status.
fn msg(s: StatusCode) -> (StatusCode, String) {
    let body = if s == StatusCode::NOT_FOUND {
        "not found"
    } else {
        "internal error"
    };
    (s, body.to_string())
}

pub async fn list_tenants(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Result<Json<Vec<inner::TenantRow>>, StatusCode> {
    let Json(rows) = inner::list_tenants(AuthenticatedUserExtractor(user.clone())).await?;
    let db = conn().await?;
    // `None` is unbounded — the Global Owner and `Scope::All`. This is the
    // lenient read-path filter by design: an unreadable grant lists everything
    // rather than showing an operator an empty fleet, which is the split
    // `scope_org_filter_checked` documents.
    Ok(Json(match scope_org_filter(&db, &user).await {
        None => rows,
        Some(orgs) => rows
            .into_iter()
            .filter(|r| orgs.contains(&r.org_id))
            .collect(),
    }))
}

pub async fn get_status(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
) -> Result<Json<oxy_oltp::api::handlers::ConnectionInfoResponse>, StatusCode> {
    let db = conn().await?;
    scope::deny_out_of_scope(&db, &user, org_id).await?;
    inner::get_status(AuthenticatedUserExtractor(user), Path(org_id)).await
}

pub async fn provision(
    user: oxy_app_core::audit::RequestActor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<inner::ProvisionRequest>,
) -> Result<Json<oxy_oltp::api::handlers::ConnectionInfoResponse>, (StatusCode, String)> {
    let db = conn().await.map_err(msg)?;
    scope::deny_out_of_scope(&db, &user, org_id)
        .await
        .map_err(msg)?;
    let writers = body.writers.clone();
    let branch = body.branch.clone();
    let out = inner::provision(
        AuthenticatedUserExtractor(user.user.clone()),
        Path(org_id),
        Json(body),
    )
    .await;
    audit_oltp(
        &db,
        &user,
        org_id,
        "oltp.provisioned",
        // `branch` only when asked for, so an ordinary provision's entry reads
        // exactly as it always has.
        match branch {
            Some(branch) => serde_json::json!({ "writers": writers, "branch": branch }),
            None => serde_json::json!({ "writers": writers }),
        },
        out.is_ok(),
    )
    .await;
    out
}

/// Audited like the rest, because this is a grant and not a display toggle.
///
/// `set_analytics_visibility(.., true)` issues `GRANT SELECT` on a tenant's
/// live business tables to `oxy_analyst_ro` — it opens those rows to every
/// analyst query in the org, and `app_*` is hidden by default precisely because
/// live app state may be regulated. It was the only mutating handler here
/// without an audit event, and the chip UI turned it into a one-click action.
pub async fn set_visibility(
    user: oxy_app_core::audit::RequestActor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<inner::VisibilityRequest>,
) -> Result<Json<oxy_oltp::api::handlers::ConnectionInfoResponse>, (StatusCode, String)> {
    let db = conn().await.map_err(msg)?;
    scope::deny_out_of_scope(&db, &user, org_id)
        .await
        .map_err(msg)?;
    let (writer, visible) = (body.writer.clone(), body.visible);
    let out = inner::set_visibility(
        AuthenticatedUserExtractor(user.user.clone()),
        Path(org_id),
        Json(body),
    )
    .await;
    audit_oltp(
        &db,
        &user,
        org_id,
        "oltp.visibility.changed",
        serde_json::json!({ "writer": writer, "visible": visible }),
        out.is_ok(),
    )
    .await;
    out
}

/// Destroying a tenant database is the least reversible act on this console, so
/// it lands in the audit log rather than only in `tracing`. `/admin/audit` is
/// where an operator goes to find out who did it; a `warn!` line is in a log
/// aggregator they may not have.
pub async fn deprovision(
    user: oxy_app_core::audit::RequestActor,
    Path(org_id): Path<Uuid>,
) -> Result<Json<oxy_oltp::api::handlers::ConnectionInfoResponse>, (StatusCode, String)> {
    let db = conn().await.map_err(msg)?;
    scope::deny_out_of_scope(&db, &user, org_id)
        .await
        .map_err(msg)?;
    // Read the database name BEFORE the delete: afterwards the row is gone and
    // the audit entry would name nothing.
    let label = oxy_oltp::api::handlers::status_for_org(&db, org_id)
        .await
        .ok()
        .map(|s| s.database)
        .unwrap_or_default();

    let out = inner::deprovision(AuthenticatedUserExtractor(user.user.clone()), Path(org_id)).await;
    audit_oltp(
        &db,
        &user,
        org_id,
        "oltp.deprovisioned",
        serde_json::json!({ "database": label }),
        out.is_ok(),
    )
    .await;
    out
}

/// Releasing ONE app's store. Audited for the same reason the whole-tenant
/// deprovision is: it drops a schema and its data, and `/admin/audit` is where
/// an operator goes to find out who did it.
///
/// The writer is recorded in the audit entry rather than only the org, because
/// the whole point of this route is that it is narrower than its neighbour —
/// an entry that said only "oltp deprovisioned" would be indistinguishable
/// from the one that destroyed the tenant.
pub async fn deprovision_writer(
    user: oxy_app_core::audit::RequestActor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<inner::DeprovisionWriterRequest>,
) -> Result<Json<oxy_oltp::api::handlers::ConnectionInfoResponse>, (StatusCode, String)> {
    let db = conn().await.map_err(msg)?;
    scope::deny_out_of_scope(&db, &user, org_id)
        .await
        .map_err(msg)?;
    let writer = body.writer.clone();

    let out = inner::deprovision_writer(
        AuthenticatedUserExtractor(user.user.clone()),
        Path(org_id),
        Json(body),
    )
    .await;
    audit_oltp(
        &db,
        &user,
        org_id,
        "oltp.writer.deprovisioned",
        serde_json::json!({ "writer": writer }),
        out.is_ok(),
    )
    .await;
    out
}

/// Handing out a DSN is a disclosure, and a writer DSN is a live write
/// credential — the one thing on this surface that leaves the console entirely.
pub async fn credentials(
    user: oxy_app_core::audit::RequestActor,
    Path(org_id): Path<Uuid>,
    Json(body): Json<inner::CredentialsRequest>,
) -> Result<Json<inner::CredentialsResponse>, (StatusCode, String)> {
    let db = conn().await.map_err(msg)?;
    scope::deny_out_of_scope(&db, &user, org_id)
        .await
        .map_err(msg)?;
    let role = body.role.clone();
    let out = inner::credentials(
        AuthenticatedUserExtractor(user.user.clone()),
        Path(org_id),
        Json(body),
    )
    .await;
    audit_oltp(
        &db,
        &user,
        org_id,
        "oltp.credential.disclosed",
        serde_json::json!({
            "role": role,
            "writable": out.as_ref().map(|Json(c)| c.writable).unwrap_or(false),
        }),
        out.is_ok(),
    )
    .await;
    out
}

/// The staging branch's status, age and the apps a reset would hit. No
/// credentials, so not audited — the same line `get_status` draws.
pub async fn get_branch(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Path((org_id, branch)): Path<(Uuid, String)>,
) -> Result<Json<oxy_oltp::api::branches::BranchStatusResponse>, (StatusCode, String)> {
    let db = conn().await.map_err(msg)?;
    scope::deny_out_of_scope(&db, &user, org_id)
        .await
        .map_err(msg)?;
    oxy_oltp::api::branches::get_branch(AuthenticatedUserExtractor(user), Path((org_id, branch)))
        .await
}

/// Reset the org's staging branch. Audited only when it happened: the
/// unconfirmed call is a preview that touched nothing, and an audit row saying
/// "reset" for it would be the one lie the log must not tell.
pub async fn reset_branch(
    user: oxy_app_core::audit::RequestActor,
    Path((org_id, branch)): Path<(Uuid, String)>,
    Json(body): Json<oxy_oltp::api::branches::ResetRequest>,
) -> Result<(StatusCode, Json<oxy_oltp::api::branches::ResetResponse>), (StatusCode, String)> {
    let db = conn().await.map_err(msg)?;
    scope::deny_out_of_scope(&db, &user, org_id)
        .await
        .map_err(msg)?;
    let confirmed = body.confirm;
    // Who the reset reaches, read BEFORE it runs: a reset that fails part way
    // may already have discarded their staging data, and the audit row is where
    // an operator goes to find out whose.
    let reach = if confirmed {
        branch_reach(&db, org_id, &branch).await
    } else {
        serde_json::Value::Null
    };
    let out = oxy_oltp::api::branches::reset_branch(
        AuthenticatedUserExtractor(user.user.clone()),
        Path((org_id, branch.clone())),
        Json(body),
    )
    .await;
    if confirmed {
        audit_oltp(
            &db,
            &user,
            org_id,
            "oltp.branch.reset",
            serde_json::json!({ "branch": branch, "reach": reach }),
            out.as_ref().is_ok_and(|(_, Json(r))| r.reset),
        )
        .await;
    }
    out
}

/// The apps and pipelines a reset of `branch` discards, for the audit row.
/// Empty rather than failing: the audit must not be what stops a reset.
async fn branch_reach(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
    branch: &str,
) -> serde_json::Value {
    let Ok(kind) = oxy_oltp::api::branches::parse_branch(branch) else {
        return serde_json::Value::Null;
    };
    match oxy_oltp::api::branches::status_for(db, org_id, kind).await {
        Ok(s) => serde_json::json!({
            "apps": s.affected_apps.iter().map(|a| a.slug.clone()).collect::<Vec<_>>(),
            "pipelines": s.affected_pipelines.iter().map(|p| p.schema.clone()).collect::<Vec<_>>(),
        }),
        Err(e) => {
            tracing::warn!("oltp branch reset: could not read its reach for the audit: {e}");
            serde_json::Value::Null
        }
    }
}

/// Best-effort: an audit write must never turn a successful provision into a
/// 500, and a failed one is already logged at `error` by `record_best_effort`.
async fn audit_oltp(
    db: &sea_orm::DatabaseConnection,
    user: &oxy_app_core::audit::RequestActor,
    org_id: Uuid,
    action: &'static str,
    metadata: serde_json::Value,
    ok: bool,
) {
    let entry = audit::AuditEntry::for_request(user, action)
        .org(org_id)
        .target("oltp_tenant", org_id.to_string(), String::new())
        .metadata(metadata);
    let entry = if ok {
        entry
    } else {
        entry.failure("handler returned an error")
    };
    audit::record_best_effort(db, entry).await;
}
