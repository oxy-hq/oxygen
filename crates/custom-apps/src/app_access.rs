//! Writing one app's access settings (its visibility and who is granted it),
//! with this process's custom-app caches flushed once the write commits.
//!
//! The write itself is [`oxy_tenancy::org_teams::service::write_access`]. That
//! crate sits below the caches, so it takes the flush as an `on_committed`
//! callback; this is the call that passes it. Both of `oxy-api-tenancy`'s doors
//! onto an app's audience call this one: the org's own route
//! (`/orgs/{id}/apps/{id}/access`) and the partner console. Without the flush,
//! a revoke keeps working on this replica until the cache's TTL.
//!
//! Lowered out of `oxy-app`'s `server::api::org_teams` (2026-11-16): it named
//! nothing of `oxy-app`'s, since both caches are this crate's ([`crate::auth`],
//! [`crate::cache`]). The staff console's door (`oxy-api-custom-apps`'
//! `admin::apps::access`) still has its own copy of this wrapper.

use entity::apps;
use http::StatusCode;
use oxy_tenancy::org_teams::dto::{AppAccessDto, SetAppAccessRequest};
use sea_orm::DatabaseConnection;
use uuid::Uuid;

/// [`oxy_tenancy::org_teams::service::write_access`], flushing the custom-app
/// access cache and app-resolution cache once it commits. Without the flush, a
/// revoke keeps working on this replica until the cache's TTL. Every surface
/// writes access through this wrapper.
pub async fn write_access(
    db: &DatabaseConnection,
    app: &apps::Model,
    actor_id: Uuid,
    req: &SetAppAccessRequest,
) -> Result<AppAccessDto, StatusCode> {
    oxy_tenancy::org_teams::service::write_access(db, app, actor_id, req, &|| {
        crate::auth::invalidate_access_cache();
        crate::cache::invalidate_app_resolution_cache_for_app(app.id);
    })
    .await
}
