//! `OxyAppAdminGuard` — gate for `/api/customer-apps/*`.
//!
//! Reads the authenticated user (inserted upstream by `auth_middleware`)
//! and checks the email against the DB-backed `app_admins` table
//! (replaces the legacy `OXY_APP_ADMINS` env-var allow-list). Returns
//! `403 FORBIDDEN` when the user isn't a global app admin and
//! `401 UNAUTHORIZED` when no authenticated user is present.
//!
//! Intentionally separate from `oxy_owner_guard`: app admins manage
//! custom-app registrations, owners manage org/billing/feature-flags
//! and add/remove app admins.

use oxy::database::client::establish_connection;

use crate::server::authz::Caller;
use crate::server::authz::globals::is_app_admin;

/// Returns `true` when the caller holds a platform grant (`app_admins`), as the
/// credential the request arrived with carries it — an API token without
/// `platform` holds none. The shipped console check the capability guards
/// difference the model against.
///
/// Wraps the cached check in [`is_app_admin`] and treats any DB
/// error as "not admin" — a transient outage should fail closed for
/// admin elevation rather than fail open.
pub async fn is_oxy_app_admin(caller: &Caller) -> bool {
    let Ok(db) = establish_connection().await else {
        tracing::warn!("is_oxy_app_admin: DB connection failed; treating as non-admin");
        return false;
    };
    match is_app_admin(&db, caller).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("is_oxy_app_admin lookup failed: {e}; treating as non-admin");
            false
        }
    }
}
