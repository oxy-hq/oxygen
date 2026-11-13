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
/// that needs a pod again has one place to say so.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &[]
}

#[cfg(test)]
mod tests {
    use super::route_roles;
    use oxy_app::surface::roles::{RouteRole, classify, install_route_declarations_for_tests_with};

    const PARTNER: &str = "33333333-3333-3333-3333-333333333333";

    /// Installed the way `oxy-server` does, then asked through the same
    /// `classify` the request path uses — so a wrong prefix fails here rather
    /// than silently falling to the FleetOk default at runtime. The one
    /// installing test in this binary: the registry is a process-wide once-cell.
    #[test]
    fn every_partner_org_route_is_served_by_the_fleet() {
        let extra = route_roles()
            .iter()
            .map(|d| (d.method, d.path.to_string(), d.role))
            .collect();
        install_route_declarations_for_tests_with(extra);

        let orgs = format!("/api/partners/{PARTNER}/orgs");
        for (method, path) in [
            ("POST", orgs.clone()),
            ("GET", orgs.clone()),
            ("PATCH", format!("{orgs}/{PARTNER}")),
        ] {
            assert_eq!(
                classify(method, &path),
                RouteRole::FleetOk,
                "{method} {path} writes only Postgres"
            );
        }
    }
}
