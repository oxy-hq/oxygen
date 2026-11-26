//! Writing one app's access settings (its visibility and who is granted it),
//! with this process's custom-app caches flushed once the write commits.
//!
//! The write itself is [`oxy_tenancy::org_teams::service::write_access`]. That
//! crate sits below the caches, so it takes the flush as an `on_committed`
//! callback; this is the call that passes it. Both of this crate's doors onto
//! an app's audience call this one: the org's own route
//! (`/orgs/{id}/apps/{id}/access`) and the partner console. Without the flush,
//! a revoke keeps working on this replica until the cache's TTL.
//!
//! Up from `oxy-custom-apps`' `app_access` (2026-11-26), whose only callers were
//! here, so that `oxy-custom-apps` no longer depends on `oxy-tenancy`
//! (`internal-docs/crate-tiers.md`, stage A); both caches are still that
//! crate's (`oxy_custom_apps::{auth, cache}`). The staff console's door
//! (`oxy-api-custom-apps`' `admin::apps::access`) still has its own copy of
//! this wrapper.

use axum::http::StatusCode;
use entity::apps;
use oxy_tenancy::org_teams::dto::{AppAccessDto, SetAppAccessRequest};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

/// [`oxy_tenancy::org_teams::service::write_access`], flushing the custom-app
/// access cache and app-resolution cache once it commits. Without the flush, a
/// revoke keeps working on this replica until the cache's TTL. Every door in
/// this crate writes access through this wrapper.
pub async fn write_access(
    db: &DatabaseConnection,
    app: &apps::Model,
    actor_id: Uuid,
    req: &SetAppAccessRequest,
) -> Result<AppAccessDto, StatusCode> {
    oxy_tenancy::org_teams::service::write_access(db, app, actor_id, req, &|| {
        oxy_custom_apps::auth::invalidate_access_cache();
        oxy_custom_apps::cache::invalidate_app_resolution_cache_for_app(app.id);
    })
    .await
}
