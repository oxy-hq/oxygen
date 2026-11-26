//! The org's team roster (`/orgs/{id}/teams/*`) and one app's visibility and
//! grants (`/orgs/{id}/apps/{id}/access`) — the HTTP half. The behavior, the
//! audit rows' names and the wire types live in the `oxy-tenancy` domain crate,
//! which custom apps and frontline use too; [`audit`] writes the rows.

pub mod app_access;
pub mod audit;
pub mod handlers;
pub mod write_access;

use oxy_tenancy::org_teams::dto;

/// The domain service, except `write_access`: that one is this crate's
/// wrapper ([`write_access`](self::write_access)), which flushes the
/// per-process custom-app caches after the write commits.
/// Named explicitly so it shadows the glob — a grant change through these
/// routes must never skip the flush.
mod service {
    pub use super::write_access::write_access;
    pub use oxy_tenancy::org_teams::service::*;
}
