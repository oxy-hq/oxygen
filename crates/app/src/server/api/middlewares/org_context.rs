use axum::extract::Path;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use chrono::Utc;
use entity::prelude::*;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::token::CredentialContext;
use oxy_auth::types::AuthenticatedUser;
use oxy_server_authz::Caller;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

// `OrgContext` + its extension-reading extractor moved to `oxy-server-authz`
// (state-agnostic authz context, consumed by the role guards that also moved).
// `org_middleware` below still loads and inserts it; re-exported here so the
// original `middlewares::org_context::{OrgContext, OrgContextExtractor}` paths
// keep resolving.
pub use oxy_server_authz::org_context::{OrgContext, OrgContextExtractor};

#[derive(serde::Deserialize)]
pub struct OrgPath {
    org_id: Uuid,
}

/// `GET /orgs/{org_id}/workspaces` — the URI here is the remainder after the
/// `/orgs/{org_id}` nest.
fn is_workspace_listing(request: &Request<axum::body::Body>) -> bool {
    request.method() == axum::http::Method::GET
        && request.uri().path().trim_end_matches('/') == "/workspaces"
}

pub async fn org_middleware(
    Path(OrgPath { org_id }): Path<OrgPath>,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    mut request: Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let caller = Caller::of(&user, request.extensions().get::<CredentialContext>());
    let ctx = resolve(&user, &caller, org_id, is_workspace_listing(&request)).await?;
    request.extensions_mut().insert(ctx);
    Ok(next.run(request).await)
}

/// The org a request names **outside its path** — a query, a body, a signed
/// OAuth `state` — resolved exactly as [`org_middleware`] resolves
/// `/orgs/{org_id}`: 404 outside the credential's reach or for an org that
/// does not exist, 403 for a non-member without a live assume-role session, and
/// the membership capped by the token's ceiling.
///
/// A per-user route that acts on an org it was handed asks this and then a role
/// guard's decision on what comes back (`role_guards::OrgAdmin::of`), rather
/// than trusting the id. Never the workspace-listing discovery exception: that
/// is one path's, and no other route is it.
pub async fn resolve_org_context(
    user: &AuthenticatedUser,
    org_id: Uuid,
) -> Result<OrgContext, StatusCode> {
    resolve(user, &Caller::from_user(user), org_id, false).await
}

/// Who `caller` is in `org_id`, as every org route sees it.
async fn resolve(
    user: &AuthenticatedUser,
    caller: &Caller,
    org_id: Uuid,
    workspace_listing: bool,
) -> Result<OrgContext, StatusCode> {
    // An API token reaches an org's own routes only through an org-wide grant
    // (design §3.2). Outside one it answers 404 — before the org is looked up,
    // so a token cannot tell an org it may not reach from one that does not
    // exist. A session and a legacy key have no ceiling here.
    //
    // A sandbox agent token holds no standing in any org (sandbox agent
    // credential design §3.2): not even the workspace discovery a grant-bound
    // token gets below.
    if caller.is_sandbox_agent() {
        return Err(StatusCode::NOT_FOUND);
    }
    let ceiling = match caller.org_ceiling(org_id) {
        Some(ceiling) => ceiling,
        // Discovery (design §4.5): a token granted only workspaces in this org may
        // still list them — the handler returns the ones it covers — and reaches
        // no other org route. It reads as the lowest role.
        None if caller.touches_org(org_id) && workspace_listing => oxy_authz::RoleCeiling::Viewer,
        None => return Err(StatusCode::NOT_FOUND),
    };

    let db = establish_connection().await.map_err(|e| {
        tracing::error!("Failed to establish DB connection: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let org = Organizations::find_by_id(org_id)
        .one(&db)
        .await
        .map_err(|e| {
            tracing::error!("Failed to query organization: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?
        .ok_or(StatusCode::NOT_FOUND)?;

    let (membership, is_global_override) = membership_in(&db, user, caller, org_id).await?;

    // The cap at the source (design §4.4): every org guard reads this role, so
    // capping it here is what makes a token of ceiling `c` on a member of role
    // `r` act as `min(r, c)` — real membership and assumed alike.
    let membership = entity::org_members::Model {
        role: oxy_server_authz::cap_org_role(membership.role, ceiling),
        ..membership
    };

    Ok(OrgContext {
        org,
        membership,
        is_global_override,
    })
}

/// The caller's membership of `org_id`: their real one, or the one a live
/// assume-role session stands in for — `true` beside a synthesized one.
async fn membership_in(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    caller: &Caller,
    org_id: Uuid,
) -> Result<(entity::org_members::Model, bool), StatusCode> {
    // A service account's standing is its own row, never `org_members` (design
    // §3.3): in its org it stands as the Member or Admin the org made it, and
    // anywhere else it does not exist. It has no assume-role path below — it
    // holds no staff or partner standing to assume with.
    let real_membership = if caller.is_service_account() {
        Some(
            caller
                .account_membership(org_id)
                .ok_or(StatusCode::NOT_FOUND)?,
        )
    } else {
        OrgMembers::find()
            .filter(entity::org_members::Column::OrgId.eq(org_id))
            .filter(entity::org_members::Column::UserId.eq(user.id))
            .one(db)
            .await
            .map_err(|e| {
                tracing::error!("Failed to query org membership: {e}");
                StatusCode::INTERNAL_SERVER_ERROR
            })?
    };

    match real_membership {
        Some(m) => Ok((m, false)),
        None => assumed_membership(db, user, caller, org_id)
            .await
            .map(|m| (m, true)),
    }
}

/// A non-member's membership: the synthetic one a live assume-role session
/// stands in for, or 403.
///
/// Not a real member — see if the caller is a platform-level operator (Global
/// Owner via OXY_OWNER, Global Admin via the `app_admins` table). If so,
/// synthesize an Owner membership so they can support / triage tenants without
/// being added as a real member. Per-org handlers that must stay
/// member-restricted check `is_global_override` and 403.
///
/// Being staff is NOT enough. The operator must have deliberately started an
/// assume-role session for THIS org — otherwise they are a plain non-member and
/// get a 403 like anyone else. This is what turns the old silent, unbounded,
/// unlogged override into an explicit, bounded, audited one. See
/// `api::admin::assume`.
///
/// TWO populations can act: Oxy staff (any org) and a partner (an assigned
/// client, with `develop_apps`). `may_act_as` decides which — and it is
/// re-checked here on every request, so revoking a partner's data-plane
/// capability kills a live session's reach at once rather than at expiry.
async fn assumed_membership(
    db: &DatabaseConnection,
    user: &AuthenticatedUser,
    caller: &Caller,
    org_id: Uuid,
) -> Result<entity::org_members::Model, StatusCode> {
    use crate::server::api::admin::assume;
    // Both reads take the CALLER: a session belongs to the credential that
    // opened it, and standing is what that credential carries.
    let live = assume::is_session_live(db, caller, org_id).await;
    let authority = if live {
        assume::may_act_as(db, caller, org_id).await
    } else {
        None
    };

    let Some(authority) = authority else {
        if live {
            tracing::warn!(
                actor_email = %user.label(),
                org_id = %org_id,
                "org_context: live session but no authority — denying (capability revoked?)"
            );
        }
        return Err(StatusCode::FORBIDDEN);
    };

    let now = Utc::now().into();
    let role = authority.org_role();
    let role_label = role.as_str();
    tracing::info!(
        actor_email = %user.label(),
        org_id = %org_id,
        ?authority,
        role = %role_label,
        "org_context: assume-role session active"
    );
    Ok(entity::org_members::Model {
        id: Uuid::nil(),
        org_id,
        user_id: user.id,
        role,
        created_at: now,
        updated_at: now,
    })
}
