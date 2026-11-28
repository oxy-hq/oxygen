//! The data-plane HTTP surface, mounted by `oxy-server`.
//!
//! - [`airhouse`] — a member's own Airhouse warehouse: `/airhouse/version` and
//!   `/airhouse/me/*`. Was `airhouse::api` (the `rest` feature) until the
//!   router left the `airhouse` crate, which is a T1 client and so may not
//!   depend on `oxy-auth` (`internal-docs/crate-tiers.md`).
//! - [`oltp`] — the caller's org's OLTP database: `/oltp/me/connection` and
//!   `/oltp/me/erd`, plus the handlers the staff console's OLTP section fences.
//!   Was `oxy_oltp::api` until crate-depth B2 cut `oxy-oltp -> oxy-auth` the
//!   same way.
//! - [`admin`] — the staff console's sections over the same two planes
//!   (`/admin/oltp*`, `/admin/orgs/{org_id}/oltp*`, `/admin/airhouse`,
//!   `/admin/workspaces/{workspace_id}/airhouse/*`), handed to the admin seam
//!   by [`admin_sections`]. `oxy-api-tenancy`'s until B2's second PR: the
//!   surface that owns a context's handlers owns its staff routes too
//!   (`internal-docs/domain-boundaries.md`, S1), and tenancy could not reach
//!   them here without a T3 -> T3 edge.

// Every surface crate declares it (`oxy-server`'s `recursion_limit_guard`):
// the release build lays out deeper futures than any PR job compiles.
#![recursion_limit = "256"]

use axum::Router;
use oxy_app_core::AppState;
use oxy_app_core::seams::AdminSection;

pub mod admin;
pub mod airhouse;
pub mod oltp;

/// The per-org data-plane provisioning fences, mounted under `/api/admin`
/// through the admin seam, each behind the capability it had in `oxy-app`
/// before the admin split (PR 4): an org's OLTP database (`/oltp`,
/// `/orgs/{org_id}/oltp*`) behind `PlatformOltp`, and a workspace's Airhouse
/// tenant (`/airhouse`, `/workspaces/{workspace_id}/airhouse/provision` and
/// `/rotate-sa`) behind `PlatformAirhouse`. One ring, two actions: the action
/// name is what lands in an audit row and in the grant UI.
///
/// Neither declares anything: the console's `/{*rest}` FleetOk wildcard
/// places them, as before.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![
        AdminSection {
            capability: oxy_server_authz::Action::PlatformOltp,
            routes: admin_oltp_routes(),
            decls: vec![],
        },
        AdminSection {
            capability: oxy_server_authz::Action::PlatformAirhouse,
            routes: admin_airhouse_routes(),
            decls: vec![],
        },
    ]
}

/// An org's OLTP database console, relative to `/admin`: the scope-fenced shim
/// over [`oltp::admin`], NOT that module directly. Its own section
/// (`PlatformOltp`), and its own function so the route catalog can seed it.
pub fn admin_oltp_routes() -> Router<AppState> {
    Router::new().merge(admin::oltp::router())
}

/// A workspace's Airhouse tenant console, relative to `/admin`: the
/// scope-fenced shim over `admin::airhouse::inner`. Its own section
/// (`PlatformAirhouse`), and its own function so the route catalog can seed it.
pub fn admin_airhouse_routes() -> Router<AppState> {
    Router::new().merge(admin::airhouse::router())
}
