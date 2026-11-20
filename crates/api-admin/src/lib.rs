//! The staff console's own crate: the cross-context staff machinery that no
//! tenant context owns (`internal-docs/domain-boundaries.md` § The admin
//! split, S1).
//!
//! Started in the admin split's PR 7 with the durable task fleet's operator
//! console ([`internal_jobs`], `/api/admin/internal-jobs/*`), out of
//! `oxy-app`'s `api::admin::internal_jobs*`. The platform audit log
//! ([`audit`], `/api/admin/audit*`), the grant table's routes
//! ([`app_admins`], `/api/admin/app-admins*`) and the assume-role routes
//! ([`assume`], `/api/assume*`) followed on 2026-11-20. `routing` follows in a
//! later PR, and the console shell (`admin::router`: the door, the capability
//! layers, `extracted_sections`) last, with the router assembly.
//!
//! Only the staff console goes here. A section about a tenant context goes to
//! that context's surface, never here by audience: orgs went to
//! `oxy-api-tenancy`, the user directory to `oxy-api-identity`, the compile
//! console to `oxy-api-previews`.
//!
//! Mounted by the `oxy-server` composition root through three seams:
//! `SurfaceSeams::admin_nests` ([`admin_nests`]), chained ahead of
//! `oxy-api-previews`' compile console so the route-role table keeps the
//! order it had in `oxy-app` (internal jobs, then compiles),
//! `SurfaceSeams::admin` ([`admin_sections`]), the audit log and the grant
//! table, merged inside `/api/admin` as they were in `oxy-app`, and
//! `SurfaceSeams::api` ([`routes`]), assume-role, merged at the root of the
//! protected tree OUTSIDE `/admin`, as `oxy-app`'s global tree merged it.
//! Every route reads or writes Postgres only (the task queue, the run,
//! workspace, audit, grant and assume-session tables; the audit anchor reads
//! S3, never a working copy), so all three are FleetOk.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`).
#![recursion_limit = "256"]

pub mod app_admins;
pub mod assume;
pub mod audit;
pub mod internal_jobs;
mod internal_jobs_dead_letter;
mod internal_jobs_failure_groups;
mod internal_jobs_fleet;
mod internal_jobs_reach;
mod internal_jobs_rows;

use axum::Router;
use oxy_app::surface::roles::RouteRole;
use oxy_app::surface::{Action, AdminNest, AdminSection};
use oxy_app_core::AppState;

/// The routes this crate hands `SurfaceSeams::api`: assume-role (`/assume`,
/// `/assume/current`, `/assume/history`), out of `oxy-app`'s
/// `admin::assume` (admin split, 2026-11-20).
///
/// **Not a section or a nest, on purpose.** Both would put the routes behind
/// the staff door and the refusal while acting: a partner, who is not staff,
/// could not reach them at all, and staff could start a session and never end
/// it, because acting closes `/admin`. The exit cannot live behind the door it
/// locks. The api seam merges it where `oxy-app`'s `build_global_routes` did:
/// the root of the protected tree, behind the authentication stack alone
/// (and internal auth on the internal port), cloud only. The handlers
/// authorize (`may_act_as`, `PlatformAudit` for the history).
///
/// No declarations: the routes are Postgres rows only, so the FleetOk default
/// is the truth, as it was for the undeclared merge in `oxy-app`.
pub fn routes() -> Router<AppState> {
    assume::router()
}

/// The staff console's sibling trees this crate hands `oxy-app`: today the
/// task fleet's operator console, nested at `/api/admin/internal-jobs` behind
/// the staff door, the refusal while acting and `PlatformOperate`.
///
/// Internal jobs is a nest rather than a section because its routes are flat
/// (no `/internal-jobs/` prefix on each), so the guards answer every path it
/// mounts, a wrong method included (a path the tree does not match is the
/// outer router's 404). Operating the durable task fleet is Oxy's own
/// machinery, not tenant data — `PlatformOperate` — and the handlers narrow by
/// grant scope themselves (`internal_jobs_reach`). `oxy-app` applies the
/// guards; this crate only names the capability.
pub fn admin_nests() -> Vec<AdminNest> {
    vec![AdminNest {
        prefix: "/internal-jobs",
        capability: Action::PlatformOperate,
        routes: internal_jobs::router(),
        role: RouteRole::FleetOk,
        why: "job admin reads the task queue tables",
    }]
}

/// The staff console's sections this crate hands `oxy-app`, merged inside
/// `/api/admin` before the refusal while acting, each behind the capability
/// `admin::router` gave it in-tree: the platform audit log (`/audit`,
/// `/audit/verify/{org_id}`, `/audit/anchor/{org_id}`) behind
/// `PlatformAudit`, and the grant table (`/app-admins`, `/app-admins/{id}`)
/// behind `PlatformGrants`.
///
/// A section, not a nest, because that is how it was mounted: a
/// `route_layer` on a router merged inside `admin::router`, ahead of the
/// refusal while acting, which is exactly what `extracted_sections` builds.
/// A nest would add its own `{prefix}` and `{prefix}/{*rest}` role rows and
/// catalog seed under a second staff door; the section adds nothing. It
/// declares nothing: the console's `/{*rest}` FleetOk wildcard places it,
/// as it did. `oxy-app` applies the guard; this crate only names the
/// capability. The audit handlers narrow by grant scope themselves
/// (`staff::scope`).
///
/// **The grant table is capability-gated, not owner-only.** It was owner-only
/// for one release, on the reasoning that "a capability that could edit the
/// grant table would let its holder widen their own grant, and the ceiling
/// would mean nothing". That objection is real and is answered by bounding the
/// write rather than withholding the capability: `may_delegate` admits only a
/// grant strictly weaker than the writer's own, so the one row a holder can
/// never touch is their own, and only the owner can mint a peer (see
/// `oxy_server_authz::staff::delegation`). **The capability gate here is a
/// door, not the control**: the handlers carry the row-level half
/// (`actor_facts` once, then `refuse(may_delegate(..))` per row), exactly as
/// scope works, and `app_scope_boundary` pins both halves. Billing stays the
/// console's one owner-only gate (`Ring::GlobalOwnerOnly`, `oxy-app`'s
/// `admin::router`). Moved here from `admin::router`'s doc with the routes.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![
        AdminSection {
            capability: Action::PlatformAudit,
            routes: audit::router(),
            decls: Vec::new(),
        },
        AdminSection {
            capability: Action::PlatformGrants,
            routes: app_admins::router(),
            decls: Vec::new(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// The nest `oxy-app`'s `admin::nests` built before the move: the same
    /// prefix, capability, placement and reason.
    #[test]
    fn the_nest_is_the_one_oxy_app_mounted() {
        let nests: Vec<_> = admin_nests()
            .iter()
            .map(|n| (n.prefix, n.capability, n.role, n.why))
            .collect();
        assert_eq!(
            nests,
            vec![(
                "/internal-jobs",
                Action::PlatformOperate,
                RouteRole::FleetOk,
                "job admin reads the task queue tables"
            )]
        );
    }

    /// The sections `oxy-app`'s `admin::router` merged before the moves: the
    /// audit log behind `PlatformAudit` and the grant table behind
    /// `PlatformGrants`, each declaring nothing.
    #[test]
    fn the_section_is_the_one_oxy_app_merged() {
        let sections = admin_sections();
        let got: Vec<_> = sections
            .iter()
            .map(|s| (s.capability, s.decls.len()))
            .collect();
        assert_eq!(
            got,
            vec![(Action::PlatformAudit, 0), (Action::PlatformGrants, 0)]
        );
    }

    /// Every audit route is mounted where the console calls it. TRACE, which
    /// none takes, answers 405 from a mounted path and 404 from an unmounted
    /// one, without running a handler or reaching a database.
    #[tokio::test]
    async fn every_audit_route_is_mounted() {
        let router = audit::router().with_state(oxy_app::surface::bare_app_state());
        let org = "00000000-0000-0000-0000-000000000001";
        for (uri, want) in [
            ("/audit".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            (
                format!("/audit/verify/{org}"),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                format!("/audit/anchor/{org}"),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            ("/audit/".to_string(), StatusCode::NOT_FOUND),
            ("/audit/verify".to_string(), StatusCode::NOT_FOUND),
            ("/auditx".to_string(), StatusCode::NOT_FOUND),
        ] {
            let got = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(got, want, "{uri}");
        }
    }

    /// Every grant-table route is mounted where the console calls it
    /// (`/api/admin/app-admins`, `/api/admin/app-admins/{id}`). TRACE, which
    /// none takes, answers 405 from a mounted path and 404 from an unmounted
    /// one, without running a handler or reaching a database.
    #[tokio::test]
    async fn every_app_admins_route_is_mounted() {
        let router = app_admins::router().with_state(oxy_app::surface::bare_app_state());
        let id = "00000000-0000-0000-0000-000000000001";
        for (uri, want) in [
            ("/app-admins".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            (format!("/app-admins/{id}"), StatusCode::METHOD_NOT_ALLOWED),
            ("/app-admins/".to_string(), StatusCode::NOT_FOUND),
            (format!("/app-admins/{id}/deeper"), StatusCode::NOT_FOUND),
            ("/app-adminsx".to_string(), StatusCode::NOT_FOUND),
        ] {
            let got = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(got, want, "{uri}");
        }
    }

    /// Every assume-role route is mounted where the web app and `oxyc assume`
    /// call it (`/api/assume`, `/api/assume/current`, `/api/assume/history`),
    /// and the api seam's router is the assume router. TRACE, which none takes,
    /// answers 405 from a mounted path and 404 from an unmounted one, without
    /// running a handler or reaching a database.
    #[tokio::test]
    async fn every_assume_route_is_mounted() {
        let router = routes().with_state(oxy_app::surface::bare_app_state());
        for (uri, want) in [
            ("/assume", StatusCode::METHOD_NOT_ALLOWED),
            ("/assume/current", StatusCode::METHOD_NOT_ALLOWED),
            ("/assume/history", StatusCode::METHOD_NOT_ALLOWED),
            ("/assume/", StatusCode::NOT_FOUND),
            ("/assume/current/x", StatusCode::NOT_FOUND),
            ("/assumex", StatusCode::NOT_FOUND),
            // Never under `/admin`: that is the door acting closes.
            ("/admin/assume", StatusCode::NOT_FOUND),
        ] {
            let got = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(got, want, "{uri}");
        }
    }

    /// No section or nest of this crate mounts assume-role: one would put the
    /// exit behind the staff door and the refusal while acting.
    #[tokio::test]
    async fn assume_role_is_never_behind_the_staff_door() {
        let state = oxy_app::surface::bare_app_state();
        let trees = admin_sections()
            .into_iter()
            .map(|s| s.routes)
            .chain(admin_nests().into_iter().map(|n| n.routes));
        for tree in trees {
            for uri in ["/assume", "/assume/current", "/assume/history"] {
                let got = tree
                    .clone()
                    .with_state(state.clone())
                    .oneshot(
                        Request::builder()
                            .method(Method::TRACE)
                            .uri(uri)
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap()
                    .status();
                assert_eq!(got, StatusCode::NOT_FOUND, "{uri} on a staff tree");
            }
        }
    }

    /// Every route the console serves is mounted where the web app calls it.
    /// TRACE, which none takes, answers 405 from a mounted path and 404 from
    /// an unmounted one, without running a handler or reaching a database.
    #[tokio::test]
    async fn every_internal_jobs_route_is_mounted() {
        let router = internal_jobs::router().with_state(oxy_app::surface::bare_app_state());
        let task = "task-0";
        for (uri, want) in [
            ("/queue-stats".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            (
                "/recent-failures".to_string(),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                "/failure-groups".to_string(),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            ("/dead-letter".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            (
                format!("/dead-letter/{task}/reenqueue"),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            (
                format!("/dead-letter/{task}"),
                StatusCode::METHOD_NOT_ALLOWED,
            ),
            ("/workers".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            ("/scheduled".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            ("/run-reaper".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            ("/run-retention".to_string(), StatusCode::METHOD_NOT_ALLOWED),
            ("/nothing-mounts-this".to_string(), StatusCode::NOT_FOUND),
        ] {
            let got = router
                .clone()
                .oneshot(
                    Request::builder()
                        .method(Method::TRACE)
                        .uri(&uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(got, want, "{uri}");
        }
    }
}
