//! `/api/admin/*` — Oxy-staff admin surface. The outer guard in
//! `router::global` is `oxy_owner_or_app_admin_guard_middleware` so both
//! OXY_OWNER staff and members of the `app_admins` table can reach most
//! admin features. One subset — billing operations — escalates to a strict
//! OXY_OWNER guard via `route_layer` below; that inner layer runs after the
//! outer permissive check and denies app-admin callers with 403. Every other
//! section escalates to the capability its surface is about, including the
//! airway admission config (`Action::PlatformOperate`).

// `app_admins` (`/app-admins*`, the grant table) is `oxy-api-admin`'s since
// the admin split (2026-11-20): a section behind `PlatformGrants`,
// arriving through `extracted`.
pub mod assume;
// `audit` (`/audit*`, the platform audit log) is `oxy-api-admin`'s since
// the admin split's audit move (2026-11-20): a section behind
// `PlatformAudit`, arriving through `extracted`.
pub mod billing;
// `compiles` (`/compiles*`, the compile boundary's operator surface) is
// `oxy-api-previews`' since the admin split's PR 6, and `internal_jobs*`
// (`/internal-jobs*`, the task fleet's operator console) `oxy-api-admin`'s
// since PR 7: each an `AdminNest` behind `PlatformOperate`, arriving through
// `SurfaceSeams::admin_nests` and nested by `router::global`.
pub mod routing;
// `usage_report` (`/usage-report*`, the weekly custom-app usage report) is
// `oxy-api-billing`'s since the admin split's PR 8b: two sections, behind
// `PlatformOperate` and `PlatformGrants`, arriving through `extracted`.
// `users_admin` and `user_scope` (the user directory, `/users*`) are
// `oxy-api-identity`'s since the admin split's PR 5: a section behind
// `PlatformUsers`, arriving through `extracted`.
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
///   - GET    /admin/internal-jobs/failure-groups
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
/// `app_admins` — the **grant table itself** — used to be the second. It is
/// capability-gated (`PlatformGrants`) and row-fenced instead, and since the admin
/// split (2026-11-20) it is `oxy-api-admin`'s section, arriving through `extracted`
/// behind that capability. Why a capability and not the owner-only guard, and why
/// that gate is a door rather than the control, is argued on
/// `oxy_api_admin::admin_sections`, beside the mount.
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
/// `/admin/internal-jobs` (`oxy-api-admin`'s since the admin split's PR 7)
/// and `/admin/compiles` (`oxy-api-previews`' since PR 6) are mounted
/// separately, as `AdminNest`s beside this router, in `router::global`:
/// internal jobs' routes were flattened during the app-admin opening.
/// `extracted` are the sections extracted surface crates contribute through
/// `SurfaceSeams::admin` — see [`extracted_sections`].
///
/// Moved out by the admin split and arriving through `extracted` behind the
/// capability they had here: Admin → Metrics' LLM cost (`/metrics/*`,
/// `PlatformOperate`) is `oxy-api-observability`'s, and the Explorer
/// (`/explorer/*`, `PlatformExplorer`) `oxy-api-threads`'. In PR 2: the
/// partner registry (`/partners*`, `PlatformPartners`) is `oxy-api-tenancy`'s,
/// its own section beside the org console's, and the CI publish tokens
/// (`/app-publish-tokens*`, `PlatformApps`) `oxy-api-custom-apps`'. In PR 4:
/// the per-org OLTP console (`/oltp`, `/orgs/{org_id}/oltp*`, `PlatformOltp`)
/// and the Airhouse fleet (`/airhouse`, `/workspaces/{workspace_id}/airhouse/
/// provision`, `PlatformAirhouse`) are `oxy-api-tenancy`'s, a section each.
/// In PR 5: the user directory (`/users*`, `PlatformUsers`) is
/// `oxy-api-identity`'s.
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
        // The platform audit log (`/audit*`, `PlatformAudit`) moved to
        // `oxy-api-admin` (admin split, 2026-11-20) and arrives through
        // `extracted` below, behind the capability it had here.
        // Every staff member's sandbox agent tokens (`PlatformOperate`) and every
        // token carrying a standing (`PlatformGrants`) moved to
        // `oxy-api-identity` and arrive through `extracted` below, behind the
        // capabilities they had here.
        // Org and workspace administration (`/orgs*`, `/workspaces*`), and an
        // org's subdomain (`/orgs/{org_id}/subdomain`), moved to
        // `oxy-api-tenancy` and arrive through `extracted` below, behind
        // `PlatformOrgs` — the capability they had here.
        // The user directory (`/users*`, `PlatformUsers`) moved to
        // `oxy-api-identity` (admin split, PR 5) and arrives through
        // `extracted` below, behind the capability it had here.
        .merge(routing::router().route_layer(cap(Action::PlatformOperate)))
        // Per-org OLTP (`PlatformOltp`) and a workspace's Airhouse tenant
        // (`PlatformAirhouse`) moved to `oxy-api-tenancy` (admin split, PR 4)
        // and arrive through `extracted` below, each its own section behind the
        // capability it had here. Provisioning creates a billable resource, so
        // an App Operator, who ships apps, must not reach either; and `cap(..)`
        // decides on a nil-org resource, so the scope-fenced shims there, not
        // the handler crates' own routers, carry the per-org fence.
        // The weekly custom-app usage report (`PlatformOperate`) and who gets it
        // (`PlatformGrants`) moved to `oxy-api-billing` (admin split, PR 8b) and
        // arrive through `extracted` below, behind the capabilities they had here.
        .merge(billing::router().route_layer(strict))
        // The grant table (`/app-admins*`, `PlatformGrants`) moved to
        // `oxy-api-admin` (admin split, 2026-11-20) and arrives through
        // `extracted` below, behind the capability it had here.
        // Sections from extracted surface crates, each behind the capability it
        // names. Merged before the acting block below, so they are refused while
        // acting exactly as the in-tree sections are.
        .merge(extracted_sections(extracted))
        .route_layer(middleware::from_fn(assume::block_admin_while_acting));

    // Assume-role itself lives at `/api/assume`, NOT here — see
    // `oxy_api_admin::assume::router` (on `SurfaceSeams::api` since the admin
    // split, 2026-11-20; the refusal above stayed in `assume`).
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
    //! plus row-fenced (see `oxy_server_authz::staff::delegation`), and `oxy-api-admin`'s
    //! section since the admin split. These tests build their own router, so they cannot
    //! notice that change — which is exactly why the assertion that the real mount matches
    //! lives in `crates/app/tests/authz/app_scope_boundary.rs` as a source scan of
    //! `oxy-api-admin`'s `admin_sections`.
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

    // The OLTP and Airhouse mount and route-inventory tests moved to
    // `oxy-api-tenancy` with the sections (admin split, PR 4): its
    // `tests/integration/admin_data_plane_mount.rs` and `admin::tests`.
}
