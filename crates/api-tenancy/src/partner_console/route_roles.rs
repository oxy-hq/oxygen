//! Which pod may serve this crate's routes, where one may not take the default.

use oxy_shared::fleet_role::RouteRoleDecl;

/// The routes in [`crate::partner_console::routes`] that may not take the FleetOk default.
///
/// Stated here because `oxy-app` owns `RoleRouter` and cannot see these
/// handlers — the same reason `oxy-api-tenancy`'s `onboarding` module carries `route_roles()`.
/// Paths are absolute: `oxy-server` merges this crate at the protected-tree
/// root, so there is no prefix to join.
///
/// None, now: creating a client org writes only Postgres (it gets no
/// workspace until its repository is connected), so every route here takes
/// the FleetOk default. Kept, and still chained by `oxy-server`, so a route
/// that needs a pod again has one place to say so. Its classification test is
/// `oxy-server`'s (`tests/integration/route_roles/tenancy_partner_console.rs`).
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &[]
}
