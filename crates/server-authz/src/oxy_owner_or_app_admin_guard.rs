//! `OxyOwnerOrAppAdminGuard` — gate that allows either OXY_OWNER or an
//! entry in the `app_admins` table.
//!
//! Used for admin surfaces that span both roles (Internal Jobs in
//! particular — operators need to triage failed tasks regardless of which
//! flavor of admin they hold). Returns `403 FORBIDDEN` when neither role
//! matches and `401 UNAUTHORIZED` when no authenticated user is present.

use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use oxy_auth::types::AuthenticatedUser;

use crate::oxy_app_admin_guard::is_oxy_app_admin;

pub async fn oxy_owner_or_app_admin_guard_middleware(
    request: axum::http::Request<axum::body::Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    let user = request
        .extensions()
        .get::<AuthenticatedUser>()
        .cloned()
        .ok_or(StatusCode::UNAUTHORIZED)?;
    // Owner is a synchronous env-var check; app-admin hits the DB. Order the check so
    // owners (the more common admin caller) get the fast path and we only hit the DB
    // for non-owners.
    //
    // Read for the CALLER, so an API token without platform standing is not staff.
    let caller = crate::Caller::of_request(request.extensions(), &user);
    let legacy = crate::globals::is_global_owner(&caller) || is_oxy_app_admin(&caller).await;

    // Platform tier through the shared model — see `Ring::GlobalAdminOrOwner` in
    // `oxy_authz`. `existing && unified`; the ring reads only the global flags.
    let facts = match oxy_platform::db::establish_connection().await {
        Ok(db) => crate::loader::load_platform_facts(&db, &caller).await,
        Err(_) => None,
    };
    let allowed = match facts {
        Some(facts) => crate::enforce(
            "guard.oxy_owner_or_app_admin",
            &facts,
            crate::Action::PlatformOps,
            &crate::Resource::platform(),
            legacy,
        ),
        // Fail-safe: unknown standing (no connection, or the lookup errored) defers to
        // the legacy verdict rather than locking operators out.
        None => legacy,
    };

    if !allowed {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(next.run(request).await)
}
