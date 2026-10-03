//! The app-access behavior lives in `oxy_tenancy::org_teams::service`; this
//! module re-exports it and adds the one thing that cannot move down:
//! dropping `oxy-app`'s per-process custom-app caches after a write.

pub use oxy_tenancy::org_teams::service::*;

use axum::http::StatusCode;
use entity::apps;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::dto::{AppAccessDto, SetAppAccessRequest};

/// [`oxy_tenancy::org_teams::service::write_access`], flushing the custom-app
/// access cache and app-resolution cache once it commits — without that, a
/// revoke keeps working on this replica until the cache's TTL. Every surface
/// writes access through this wrapper.
pub async fn write_access(
    db: &DatabaseConnection,
    app: &apps::Model,
    actor_id: Uuid,
    req: &SetAppAccessRequest,
) -> Result<AppAccessDto, StatusCode> {
    oxy_tenancy::org_teams::service::write_access(db, app, actor_id, req, &|| {
        crate::server::api::custom_apps_auth::invalidate_access_cache();
        crate::server::api::custom_apps_cache::invalidate_app_resolution_cache();
    })
    .await
}
