//! The org's team roster (`/orgs/{id}/teams/*`) and one app's visibility and
//! grants (`/orgs/{id}/apps/{id}/access`) — the HTTP half. The behavior, audit
//! rows and wire types live in the `oxy-tenancy` domain crate, which custom
//! apps and frontline use too.

pub mod app_access;
pub mod handlers;

use oxy_tenancy::org_teams::{audit, dto};

/// The domain service, except `write_access`: that one is `oxy-custom-apps`'
/// wrapper, which flushes the per-process custom-app caches after the write
/// commits.
/// Named explicitly so it shadows the glob — a grant change through these
/// routes must never skip the flush.
mod service {
    pub use oxy_custom_apps::app_access::write_access;
    pub use oxy_tenancy::org_teams::service::*;
}
