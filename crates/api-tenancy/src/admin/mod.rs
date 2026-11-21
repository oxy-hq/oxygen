//! The staff console's org and workspace sections (`/api/admin/orgs*`,
//! `/api/admin/workspaces*`), mounted through the admin seam — see
//! [`crate::admin_sections`]. Moved from `oxy-app`'s `admin::{orgs_admin,
//! workspaces_admin}`, and `org_subdomains` (an org's bare subdomain, the
//! staff half of `crate::org_subdomain`) from `oxy-app`'s
//! `admin::org_subdomains` (admin split, PR 1). The partner registry
//! (`/api/admin/partners*`, staff granting and governing partnerships — the
//! staff half of `crate::partner_console`) from `oxy-app`'s `admin::partners`
//! (admin split, PR 2), its own section behind `PlatformPartners`.
//!
//! The per-org data-plane provisioning fences, from `oxy-app`'s
//! `admin::{oltp, airhouse}` (admin split, PR 4): `oltp` (an org's OLTP
//! database, behind `PlatformOltp`) and `airhouse` (a workspace's Airhouse
//! tenant, behind `PlatformAirhouse`), each its own section. Both are shims:
//! the handlers live in `oxy-oltp` and `airhouse`, which cannot read
//! `app_admins`, so the routes are declared here and each org- or
//! workspace-keyed one is fenced before it delegates. Their routes, as the
//! console serves them:
//!   - GET    /admin/airhouse
//!   - POST   /admin/workspaces/{workspace_id}/airhouse/provision
//!   - POST   /admin/workspaces/{workspace_id}/airhouse/rotate-sa
//!   - GET    /admin/oltp
//!   - GET    /admin/orgs/{org_id}/oltp
//!   - POST   /admin/orgs/{org_id}/oltp/test-connection
//!   - POST   /admin/orgs/{org_id}/oltp/provision
//!   - POST   /admin/orgs/{org_id}/oltp/credentials
//!   - POST   /admin/orgs/{org_id}/oltp/visibility
//!   - POST   /admin/orgs/{org_id}/oltp/deprovision-writer
//!   - DELETE /admin/orgs/{org_id}/oltp
//!   - GET    /admin/orgs/{org_id}/oltp/branches/{branch}
//!   - POST   /admin/orgs/{org_id}/oltp/branches/{branch}/reset
//!   - POST   /admin/orgs/{org_id}/oltp/audit
//!   - POST   /admin/orgs/{org_id}/oltp/roles/{role}/rotate

pub mod airhouse;
pub mod oltp;
pub mod org_subdomains;
pub mod orgs;
pub mod partners;
pub mod workspaces;

#[cfg(test)]
mod tests {
    /// The route list in this module's header must name every route the fenced
    /// shims actually mount. Moved from `oxy-app`'s `admin::tests` with the
    /// shims (admin split, PR 4), where the inventory was that module's header.
    ///
    /// That header is the inventory an operator greps, and it drifted twice in
    /// one review — the routes were missing from it, and the path recorded in
    /// `internal-docs/admin-surfaces.md` was `/admin/airhouse/{id}/provision`
    /// when the real one is `/admin/workspaces/{id}/airhouse/provision`. A
    /// wrong path in an inventory is worse than an absent one: it reads as
    /// checked.
    #[test]
    fn the_header_inventory_names_every_shim_route() {
        for shim in [include_str!("airhouse.rs"), include_str!("oltp.rs")] {
            assert_header_lists_routes_of(shim);
        }
    }

    fn assert_header_lists_routes_of(shim: &str) {
        // Everything above this test module: the module docs and the route
        // inventory they carry. Deliberately NOT the whole file — this test
        // quotes paths in its own doc comment, and searching those would let
        // it pass on the strength of a comment about the bug.
        let header = include_str!("mod.rs")
            .split_once("#[cfg(test)]")
            .expect("the inventory sits above the tests")
            .0;
        assert!(
            header.contains("/admin/orgs"),
            "the header slice missed the route inventory entirely"
        );

        // Every `"/..."` literal inside the shim's `router()`. Matching
        // `.route("` misses the shape rustfmt gives a long call — the path
        // ends up on its own line — which would have left this guard checking
        // one of the two routes and passing.
        let body = shim
            .split_once("pub fn router<S>()")
            .expect("the shim mounts its routes in `router`")
            .1;
        let body = body.split_once("\nasync fn ").map_or(body, |(b, _)| b);
        let mounted: Vec<&str> = body
            .match_indices('"')
            .filter_map(|(i, _)| {
                let rest = &body[i + 1..];
                let end = rest.find('"')?;
                let lit = &rest[..end];
                lit.starts_with('/').then_some(lit)
            })
            .collect();
        assert!(
            !mounted.is_empty(),
            "the `.route(..)` scan matched nothing — the mount syntax changed \
             and this guard is now vacuous"
        );
        for path in mounted {
            // The admin seam supplies the `/admin` prefix at the mount, and
            // the header records the externally visible path — which is the
            // one an operator greps for and the one the docs got wrong.
            let external = format!("/admin{path}");
            assert!(
                header.contains(&external),
                "`{external}` is mounted but absent from the route list at the \
                 top of this module"
            );
        }
    }
}
