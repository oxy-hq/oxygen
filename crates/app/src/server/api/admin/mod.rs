//! `/api/admin/*` — Oxy-staff admin surface. The outer guard in
//! `router::global` is `oxy_owner_or_app_admin_guard_middleware` so both
//! OXY_OWNER staff and members of the `app_admins` table can reach most
//! admin features. One subset — billing operations — escalates to a strict
//! OXY_OWNER guard via `route_layer` below; that inner layer runs after the
//! outer permissive check and denies app-admin callers with 403. Every other
//! section escalates to the capability its surface is about, including the
//! airway admission config (`Action::PlatformOperate`).

pub mod airhouse;
pub mod app_admins;
pub mod app_publish_tokens;
pub mod assume;
pub mod audit;
pub mod billing;
pub mod compiles;
pub mod delegation;
pub mod explorer;
pub mod internal_jobs;
mod internal_jobs_dead_letter;
mod internal_jobs_fleet;
mod internal_jobs_reach;
mod internal_jobs_rows;
pub mod metrics;
pub(crate) mod metrics_pricing;
pub(crate) mod metrics_rollup;
pub mod oltp;
pub mod org_filter;
pub mod org_subdomains;
pub mod partners;
pub mod routing;
pub mod scope;
pub mod usage_report;
mod user_scope;
pub mod users_admin;
// `workspace_health` is the `oxy-api-workspace-health` surface's (an `AdminSection`
// under `PlatformOperate`), its domain `oxy-workspace-health`.

use axum::Router;
use axum::middleware;

use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

use crate::server::api::middlewares::{oxy_owner_guard, platform_cap_guard};
use crate::server::authz::Action;
use crate::server::feature_flags;
use crate::server::router::{AdminSection, AppState};

/// Admin routes are flat under `/api/admin/*` after the 2026-04-28 redesign.
/// Endpoints:
///   - GET    /admin/orgs?status=...
///   - GET    /admin/billing/prices
///   - GET    /admin/orgs/{org_id}/billing/subscription
///   - POST   /admin/orgs/{org_id}/billing/provision-subscription
///   - POST   /admin/orgs/{org_id}/billing/provision-checkout
///   - GET    /admin/orgs/{org_id}/billing/checkout
///   - POST   /admin/orgs/{org_id}/billing/checkout/resend
///   - POST   /admin/orgs/{org_id}/billing/checkout/cancel
///   - POST   /admin/orgs/{org_id}/billing/resync
///   - GET    /admin/feature-flags
///   - GET    /admin/feature-flags/evaluate
///   - PATCH  /admin/feature-flags/{key}
///   - GET    /admin/app-admins
///   - POST   /admin/app-admins
///   - DELETE /admin/app-admins/{id}
///   - GET    /admin/standing-tokens
///   - POST   /admin/standing-tokens/{id}/revoke
///   - GET    /admin/internal-jobs/queue-stats
///   - GET    /admin/internal-jobs/recent-failures
///   - GET    /admin/internal-jobs/dead-letter
///   - POST   /admin/internal-jobs/dead-letter/{task_id}/reenqueue
///   - DELETE /admin/internal-jobs/dead-letter/{task_id}
///   - GET    /admin/internal-jobs/workers
///   - GET    /admin/internal-jobs/scheduled
///   - POST   /admin/internal-jobs/run-reaper
///   - GET    /admin/orgs-meta
///   - GET    /admin/orgs/{org_id}/detail
///   - PATCH  /admin/orgs/{org_id}
///   - DELETE /admin/orgs/{org_id}
///   - POST   /admin/orgs/{org_id}/transfer-ownership
///   - GET    /admin/users
///   - GET    /admin/users/{user_id}
///   - PATCH  /admin/users/{user_id}/status
///   - GET    /admin/users/{user_id}/org-memberships
///   - POST   /admin/users/{user_id}/org-memberships
///   - PATCH  /admin/users/{user_id}/org-memberships/{org_id}
///   - DELETE /admin/users/{user_id}/org-memberships/{org_id}
///   - GET    /admin/workspaces-meta
///   - GET    /admin/workspaces/{workspace_id}/detail
///   - PATCH  /admin/workspaces/{workspace_id}
///   - DELETE /admin/workspaces/{workspace_id}
///   - POST   /admin/workspaces/{workspace_id}/transfer-org
///   - GET    /admin/usage-report
///   - GET    /admin/usage-report/email-preference
///   - PUT    /admin/usage-report/email-preference
///   - POST   /admin/usage-report/send-to-me
///   - GET    /admin/usage-report/recipients
///   - PUT    /admin/usage-report/recipients/{email}
///   - POST   /admin/workspace-health/{workspace_id}/eval
///   - GET    /admin/workspace-health/{workspace_id}/history
///   - GET    /admin/airway/config
///   - PUT    /admin/airway/config/{source_kind}
///   - DELETE /admin/airway/config/{source_kind}
///   - PUT    /admin/airway/config/{source_kind}/workspaces/{workspace_id}
///   - DELETE /admin/airway/config/{source_kind}/workspaces/{workspace_id}
///   - GET    /admin/airway/config/{source_kind}/preview
///   - GET    /admin/airway/deployment-config
///   - PUT    /admin/airway/deployment-config
///   - DELETE /admin/airway/deployment-config
///   - GET    /admin/airhouse
///   - POST   /admin/workspaces/{workspace_id}/airhouse/provision
///   - GET    /admin/oltp
///   - GET    /admin/orgs/{org_id}/oltp
///   - POST   /admin/orgs/{org_id}/oltp/test-connection
///   - POST   /admin/orgs/{org_id}/oltp/provision
///   - POST   /admin/orgs/{org_id}/oltp/credentials
///   - POST   /admin/orgs/{org_id}/oltp/visibility
///   - POST   /admin/orgs/{org_id}/oltp/deprovision-writer
///   - DELETE /admin/orgs/{org_id}/oltp
///   - GET    /admin/orgs/{org_id}/oltp/branches/{branch}
///   - POST   /admin/orgs/{org_id}/oltp/branches/{branch}/reset
///
/// Admin routes. The outer nest layer in `router::global` is the **door**
/// (`oxy_owner_or_app_admin_guard`): it answers "are you Oxy staff at all". Each
/// sub-router below then escalates to the capability its surface is actually about,
/// via `route_layer` — the inner layer runs after the outer one, so a request that
/// passed the door still gets a 403 here without the capability.
///
/// That per-section escalation is not new machinery: `billing` and `app_admins` have
/// always escalated to strict OXY_OWNER this way. What changed is that the *rest* of
/// the console used to escalate to nothing, so any staff standing reached all of it —
/// which is why an app publisher had the same authority as someone entitled to delete
/// a tenant. `require(Action::Platform*)` generalises the pattern that was already here.
///
/// One gate remains owner-only rather than capability-gated, deliberately:
/// * `billing` — the Billing queue, `Ring::GlobalOwnerOnly`.
///
/// `app_admins` — the **grant table itself** — used to be the second, on the reasoning
/// that "a capability that could edit the grant table would let its holder widen their
/// own grant, and the ceiling would mean nothing". That objection is real and is
/// answered by bounding the write rather than withholding the capability: `may_delegate`
/// admits only a grant strictly weaker than the writer's own, so the one row a holder
/// can never touch is their own, and only the owner can mint a peer. See
/// `admin::delegation`. **The capability gate here is a door, not the control** — the
/// handlers carry the row-level half (`actor_facts` once, then `refuse(may_delegate(..))`
/// per row), exactly as scope works.
///
/// **Scope is not enforced here** — see `platform_cap_guard`. A scoped operator passes
/// these gates and the handler filters its rows.
///
/// Airway's section (now `oxy-api-airway`, through the admin seam) is deliberately *not* a
/// third owner-only gate: it is deployment-wide operational config, so it takes
/// `Action::PlatformOperate` beside `workspace_health` / `routing` / `metrics`. The two
/// exceptions above are exceptions for reasons that do not apply to it — billing is
/// `Ring::GlobalOwnerOnly`, and `app_admins` is the grant table itself, where a capability
/// could widen its own holder.
///
/// `internal_jobs::router()` is mounted separately at `/admin/internal-jobs`
/// in `router::global` because its routes were flattened during the
/// app-admin opening.
/// `extracted` are the sections extracted surface crates contribute through
/// `SurfaceSeams::admin` — see [`extracted_sections`].
pub(crate) fn router(extracted: Vec<AdminSection>) -> Router<AppState> {
    // route_layer applied per sub-router so only billing gets the strict
    // owner guard; everything else escalates to a capability, and anything
    // not named here runs only under the outer permissive guard.
    let strict = middleware::from_fn(oxy_owner_guard::oxy_owner_guard_middleware);
    let cap = |action| middleware::from_fn(platform_cap_guard::require(action));

    // The staff surface. Everything here is refused while the caller is acting as
    // a tenant: you cannot wield staff powers and wear a customer's identity in
    // the same breath. Ending the session (below) restores all of it.
    let staff_surface = feature_flags::routes::router()
        .route_layer(cap(Action::PlatformOperate))
        // The custom-app registry is NOT merged here: it is the `/customer-apps`
        // surface in `router::global`, behind the one `apps::guarded` stack.
        .merge(audit::router().route_layer(cap(Action::PlatformAudit)))
        .merge(app_publish_tokens::router().route_layer(cap(Action::PlatformApps)))
        // Every staff member's sandbox agent tokens (`PlatformOperate`) and every
        // token carrying a standing (`PlatformGrants`) moved to
        // `oxy-api-identity` and arrive through `extracted` below, behind the
        // capabilities they had here.
        .merge(explorer::router().route_layer(cap(Action::PlatformExplorer)))
        .merge(metrics::router().route_layer(cap(Action::PlatformOperate)))
        // Org and workspace administration (`/orgs*`, `/workspaces*`) moved to
        // `oxy-api-tenancy` and arrive through `extracted` below, behind
        // `PlatformOrgs` — the capability they had here.
        .merge(org_subdomains::router().route_layer(cap(Action::PlatformOrgs)))
        .merge(users_admin::router().route_layer(cap(Action::PlatformUsers)))
        .merge(routing::router().route_layer(cap(Action::PlatformOperate)))
        // Per-org OLTP: provisioning creates a billable project at the
        // provider, so it sits behind `OperatePlatform`, not the staff door —
        // an App Operator ships apps and must not spend money here.
        // The scope-fenced shim, NOT `oxy_oltp::api::admin::router()` directly.
        // `cap(..)` decides on a nil-org resource, so it lets a bounded
        // `global_admin` through for every org; `oltp::router` adds the
        // per-org fence. See that module's header.
        .merge(oltp::router().route_layer(cap(Action::PlatformOltp)))
        // Same RING as OLTP — both are "operate the platform's data plane on a
        // tenant's behalf" — but its own ACTION, because the action name is what
        // lands in an audit row and in the grant UI.
        .merge(airhouse::router().route_layer(cap(Action::PlatformAirhouse)))
        // The weekly custom-app usage report. A cross-tenant read, so the handlers
        // narrow it to the orgs the caller's grant reaches (`scope::list_scope`);
        // this layer cannot. The same capability decides who is emailed the report
        // (`usage_report::delivery::AUDIENCE`), so the mail's link always opens.
        .merge(usage_report::router().route_layer(cap(Action::PlatformOperate)))
        // Who gets it, and switching it off for another person: the staff list,
        // so the capability that administers staff access.
        .merge(usage_report::recipients::router().route_layer(cap(Action::PlatformGrants)))
        .merge(partners::router().route_layer(cap(Action::PlatformPartners)))
        .merge(billing::router().route_layer(strict))
        .merge(app_admins::router().route_layer(cap(Action::PlatformGrants)))
        // Sections from extracted surface crates, each behind the capability it
        // names. Merged before the acting block below, so they are refused while
        // acting exactly as the in-tree sections are.
        .merge(extracted_sections(extracted))
        .route_layer(middleware::from_fn(assume::block_admin_while_acting));

    // Assume-role itself lives at `/api/assume`, NOT here — see `assume::router`.
    // It has to be reachable while acting (that's where the exit is) and by
    // partners (who are not staff and would be 403'd by this surface's guard).
    staff_surface
}

/// The sections extracted surfaces contribute, each wrapped in the capability
/// guard it names. `oxy-app` applies the guard rather than the surface, so a
/// section cannot reach the console without one.
fn extracted_sections(sections: Vec<AdminSection>) -> Router<AppState> {
    sections.into_iter().fold(Router::new(), |router, section| {
        router.merge(
            section
                .routes
                .route_layer(middleware::from_fn(platform_cap_guard::require(
                    section.capability,
                ))),
        )
    })
}

/// Which pod serves each route [`router`] mounts, relative to its `/admin` nest.
///
/// FleetOk wholesale: the console is Postgres CRUD, and the one filesystem read
/// under it (`reconcile.yml`, for workspace health) runs on the worker, not the
/// request path. The exceptions are declared beside their routes and win on
/// specificity over the wildcard.
pub(crate) fn router_roles() -> &'static [RouteRoleDecl] {
    const ROLES: &[RouteRoleDecl] = &[
        RouteRoleDecl {
            method: "*",
            path: "",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "*",
            path: "/{*rest}",
            role: RouteRole::FleetOk,
        },
    ];
    ROLES
}

#[cfg(test)]
mod tests {
    //! Regression tests for the route-layer escalation pattern used by `billing`.
    //! These pin the property that even when the outer permissive layer in
    //! `router::global` admits a Global Admin (a non-Owner staff member), a route
    //! nested under `route_layer(oxy_owner_guard_middleware)` still rejects with 403.
    //!
    //! `app_admins` was the second such surface and is no longer: it is capability-gated
    //! plus row-fenced (see `admin::delegation`). These tests build their own router, so
    //! they cannot notice that change — which is exactly why the assertion that the real
    //! mount matches lives in `crates/app/tests/app_scope_boundary.rs` as a source scan.
    //! A fixture that mounts its own probe under `strict` proves the *middleware* works
    //! and nothing whatsoever about what ships.
    //!
    //! The escalation does NOT touch the database — `oxy_owner_guard`
    //! consults only the `OXY_OWNER` env var — so we can pin the layering
    //! behavior with a plain Tower service test rather than a full
    //! integration harness.
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use entity::users::UserStatus;
    use oxy_auth::types::AuthenticatedUser;
    use tower::ServiceExt;
    use uuid::Uuid;

    /// Set / unset env vars for the duration of one test. Necessary
    /// because the guard reads `OXY_OWNER` at request time, not boot.
    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.prev {
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    fn stub_user(email: &str) -> AuthenticatedUser {
        AuthenticatedUser {
            id: Uuid::new_v4(),
            email: Some(email.to_string()),
            name: "stub".to_string(),
            picture: None,
            status: UserStatus::Active,
            credential: None,
        }
    }

    /// Build the same router shape as `admin::router()` minus the leaf
    /// handlers (DB-heavy). One stub handler per surface so we can assert
    /// per-path response codes.
    fn test_router() -> Router {
        let strict = middleware::from_fn(oxy_owner_guard::oxy_owner_guard_middleware);
        let billing = Router::new()
            .route("/billing/probe", get(|| async { StatusCode::OK }))
            .route_layer(strict.clone());
        // `partners` and `app-admins` mount WITHOUT the strict layer — a Global Admin
        // must reach both. (Partners is the regression guard for the 403-on-partners
        // bug; app-admins is capability-gated now, with the delegation bound doing the
        // narrowing inside the handler rather than at the door.)
        let open = Router::new()
            .route("/feature-flags/probe", get(|| async { StatusCode::OK }))
            .route("/partners/probe", get(|| async { StatusCode::OK }))
            .route("/app-admins/probe", get(|| async { StatusCode::OK }));
        Router::new().merge(billing).merge(open)
    }

    async fn request_as(router: Router, path: &str, user: Option<AuthenticatedUser>) -> StatusCode {
        let mut req = Request::builder().uri(path).body(Body::empty()).unwrap();
        if let Some(u) = user {
            req.extensions_mut().insert(u);
        }
        router.oneshot(req).await.unwrap().status()
    }

    /// The guard on an extracted section is `oxy-app`'s, not the surface's:
    /// the same probe answers 200 bare and 401 once `extracted_sections` has
    /// wrapped it in its capability. (The outer staff guard also answers 401,
    /// so the real-router test cannot tell the two apart — this one can.)
    #[tokio::test]
    async fn extracted_sections_apply_the_capability_they_name() {
        let probe = || Router::new().route("/probe", get(|| async { "ok" }));
        let bare = probe().with_state(test_app_state());
        assert_eq!(request_as(bare, "/probe", None).await, StatusCode::OK);

        let guarded = extracted_sections(vec![AdminSection {
            capability: Action::PlatformOrgs,
            routes: probe(),
            decls: Vec::new(),
        }])
        .with_state(test_app_state());
        assert_eq!(
            request_as(guarded, "/probe", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    /// A non-owner caller gets 403 on billing, but 200 on the non-escalated paths.
    /// This pins the route_layer escalation itself.
    #[tokio::test]
    async fn strict_layer_rejects_non_owner_on_billing() {
        let _g = EnvGuard::set("OXY_OWNER", "owner@example.com");
        let app = test_router();

        let admin = stub_user("admin@example.com");

        assert_eq!(
            request_as(app.clone(), "/billing/probe", Some(admin.clone())).await,
            StatusCode::FORBIDDEN,
            "Global Admin must NOT reach billing through the strict route_layer"
        );
        assert_eq!(
            request_as(app.clone(), "/app-admins/probe", Some(admin.clone())).await,
            StatusCode::OK,
            "app_admins is capability-gated, not owner-strict: a Global Admin reaches \
             the console and `may_delegate` decides which rows they may write"
        );
        assert_eq!(
            request_as(app.clone(), "/partners/probe", Some(admin.clone())).await,
            StatusCode::OK,
            "Global Admin MUST reach partners — provisioning is not owner-strict"
        );
        assert_eq!(
            request_as(app, "/feature-flags/probe", Some(admin)).await,
            StatusCode::OK,
            "non-escalated paths stay reachable by any authenticated caller"
        );
    }

    /// Sanity: an Owner reaches every surface — confirms the strict
    /// layer doesn't accidentally reject owners.
    #[tokio::test]
    async fn strict_layer_allows_owner_everywhere() {
        let _g = EnvGuard::set("OXY_OWNER", "owner@example.com");
        let app = test_router();
        let owner = stub_user("owner@example.com");

        for path in [
            "/billing/probe",
            "/app-admins/probe",
            "/feature-flags/probe",
        ] {
            assert_eq!(
                request_as(app.clone(), path, Some(owner.clone())).await,
                StatusCode::OK,
                "Global Owner must reach {path}"
            );
        }
    }

    /// Missing auth extension → 401 from the strict guard. Protects
    /// against a deploy that accidentally drops the auth middleware.
    #[tokio::test]
    async fn strict_layer_rejects_unauthenticated() {
        let _g = EnvGuard::set("OXY_OWNER", "owner@example.com");
        let app = test_router();

        assert_eq!(
            request_as(app, "/billing/probe", None).await,
            StatusCode::UNAUTHORIZED
        );
    }

    fn test_app_state() -> AppState {
        AppState {
            enterprise: false,
            internal: false,
            mode: oxy_app_core::serve_mode::ServeMode::Cloud,
            observability: None,
            startup_cwd: std::path::PathBuf::new(),
            preagg_cache: None,
            preagg_renewal_threshold_secs: None,
            agentic_state: None,
            semantic_layer_cache: crate::server::router::workspace_cache::new_semantic_layer_cache(
            ),
            semantic_engine_cache:
                crate::server::router::workspace_cache::new_semantic_engine_cache(),
        }
    }

    /// The same guard for the per-org OLTP routes, which shipped 404ing.
    ///
    /// They were declared as `/admin/orgs/{id}/oltp` while `admin::router()` is
    /// already nested under `/admin` in `router::global` — so the real path was
    /// `/admin/admin/orgs/...`. It compiled, the handler was reachable in
    /// principle, and every request 404'd. Nothing but driving the real router
    /// catches a doubled prefix.
    #[tokio::test]
    async fn oltp_admin_routes_are_mounted_on_the_real_router() {
        let real_router = router(Vec::new()).with_state(test_app_state());
        let org = "d9830be4-c6a4-4c1c-9c1e-000000000002";

        for (method, path) in [
            // All seven, not the original three. The three that matter most to
            // get wrong sit together: visibility grants analytics read access to
            // a tenant's live tables, `deprovision-writer` drops one app's
            // schema, and the DELETE destroys the whole database. The last two
            // are one path segment apart and differ by the entire tenant, which
            // is why both are pinned here rather than trusted to review.
            // `GET /oltp` additionally changed crates when the scope-fenced shim
            // took over the mount.
            ("GET", "/oltp".to_string()),
            ("GET", format!("/orgs/{org}/oltp")),
            ("POST", format!("/orgs/{org}/oltp/test-connection")),
            ("POST", format!("/orgs/{org}/oltp/provision")),
            ("POST", format!("/orgs/{org}/oltp/credentials")),
            ("POST", format!("/orgs/{org}/oltp/visibility")),
            ("POST", format!("/orgs/{org}/oltp/deprovision-writer")),
            ("DELETE", format!("/orgs/{org}/oltp")),
            // The staging branch: its status, and the reset that discards every
            // app's staging data in the org.
            ("GET", format!("/orgs/{org}/oltp/branches/staging")),
            ("POST", format!("/orgs/{org}/oltp/branches/staging/reset")),
            // Airhouse rides the same mount and would 404 the same way.
            ("GET", "/airhouse".to_string()),
            ("POST", format!("/workspaces/{org}/airhouse/provision")),
        ] {
            let req = Request::builder()
                .method(method.as_ref() as &str)
                .uri(&path)
                .body(Body::empty())
                .unwrap();
            let resp = real_router.clone().oneshot(req).await.unwrap();
            assert_ne!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{method} {path} must be mounted on admin::router() — a 404 here means the \
                 merge is missing or the route carries its own /admin prefix"
            );
        }

        // Control: the doubled-prefix shape must NOT match, or the assertions
        // above would pass for the very bug they exist to catch.
        let doubled = Request::builder()
            .uri(format!("/admin/orgs/{org}/oltp").as_str())
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            real_router.oneshot(doubled).await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "the /admin/admin/... shape must not be mounted"
        );
    }

    /// The route list in this file's header must name every route the fenced
    /// shims actually mount.
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
        // Everything above `router()`: the module list and the route inventory
        // that documents it. Deliberately NOT the whole file — the tests below
        // quote paths in their own doc comments, and searching those would let
        // this pass on the strength of a comment about the bug.
        let header = include_str!("mod.rs")
            .split_once("pub(crate) fn router()")
            .expect("the inventory sits above `router()`")
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
            // `admin::router()` supplies the `/admin` prefix at the mount, and
            // the header records the externally visible path — which is the
            // one an operator greps for and the one the docs got wrong.
            let external = format!("/admin{path}");
            assert!(
                header.contains(&external),
                "`{external}` is mounted but absent from the route list at the \
                 top of this file"
            );
        }
    }

    /// The Airhouse admin routes, driven through the REAL `router()`.
    ///
    /// `test_router()` elsewhere in this file is a hand-rolled stub that
    /// re-declares probe routes, so it cannot catch a missing `.merge(..)`
    /// line. A 404 on a real path means the route is not mounted; the bogus
    /// sibling below stays 404 as the control, because `route_layer` only wraps
    /// MATCHED routes and an unmatched path bypasses it to axum's fallback.
    #[tokio::test]
    async fn airhouse_admin_routes_are_mounted_on_the_real_router() {
        let real_router = router(Vec::new()).with_state(test_app_state());
        let ws = "3c6e0b8a-9c15-224a-8236-000000000001";

        for (method, path) in [
            ("GET", "/airhouse".to_string()),
            ("POST", format!("/workspaces/{ws}/airhouse/provision")),
        ] {
            let req = Request::builder()
                .method(method)
                .uri(&path)
                .body(Body::empty())
                .unwrap();
            let resp = real_router.clone().oneshot(req).await.unwrap();
            assert_ne!(
                resp.status(),
                StatusCode::NOT_FOUND,
                "{method} {path} is not mounted"
            );
        }

        // The control: an unmounted sibling must still 404, or the assertions
        // above would pass for the very bug they exist to catch.
        let bogus = Request::builder()
            .uri("/airhouse/not-a-route")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            real_router.oneshot(bogus).await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "an unmounted path must 404, otherwise this test proves nothing"
        );
    }
}
