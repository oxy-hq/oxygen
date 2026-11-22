//! Billing's HTTP surface (`internal-docs/domain-boundaries.md` § The admin
//! split).
//!
//! Started in the admin split's PR 8b with the weekly custom-app usage
//! report's surface ([`usage_report`]: the console's routes, who gets it,
//! the delivery and the weekly pass), out of `oxy-app`'s
//! `api::admin::usage_report`. The report's domain half is `oxy-usage-report`,
//! below this crate. PR 9 brought the rest of billing's routes, out of
//! `oxy-app`: the staff billing console ([`admin::billing`], out of
//! `admin::billing`) and the tenant billing routes with the Stripe webhook
//! ([`billing`], out of `api::billing`). The service gate (`billing_service`,
//! `billing_disabled`, in `oxy_billing::gate` since decompose B1) arrives
//! through `oxy_app::surface::billing`; the past-due dispatcher
//! ([`billing::notify`]) stayed in `oxy-app` at PR 9 and came here in
//! decompose B2, with its only callers.
//!
//! Mounted by the `oxy-server` composition root through four seams:
//!
//! - `SurfaceSeams::admin` ([`admin_sections`]): three sections merged inside
//!   `/api/admin`, ahead of the refusal while acting, behind the gates
//!   `admin::router` gave them in-tree. `oxy-app` applies the staff door, the
//!   refusal and the gate; this crate only names it.
//! - `SurfaceSeams::api` ([`routes`], [`route_roles`]): the tenant's
//!   `/orgs/{org_id}/billing/*`, at the protected root, behind
//!   `org_middleware` only, as `build_org_routes` mounted it: billing is the
//!   one org tree a paywalled org may reach, so no subscription guard.
//! - `SurfaceSeams::public` ([`public_routes`], [`public_route_roles`]):
//!   `POST /webhooks/stripe`, outside the auth stack; the handler verifies
//!   Stripe's signature over the raw body instead.
//! - `SurfaceSeams::global_ticks` ([`global_ticks`]): the weekly pass, at
//!   `TickPass::EveryLoop`, which runs it where `router::recovery` called it
//!   inline: once per iteration of the global driver's loop, on every process
//!   that runs the driver, whatever that node claimed.
//!
//! Every route reads or writes Postgres (the report, preference and delivery
//! rows, the staff grants, the Stripe customer and subscription rows), calls
//! Stripe, and sends mail; nothing reads a working copy. The console's FleetOk
//! wildcard places the admin sections, which declare nothing; the tenant
//! routes and the webhook declare the FleetOk rows `nest_all` and
//! `route_fleet` declared for them in `oxy-app`.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`).
#![recursion_limit = "256"]

pub mod admin;
pub mod billing;
pub mod usage_report;

use axum::Router;
use axum::routing::post;
use oxy_app::surface::seams::{GlobalTickSeam, TickPass};
use oxy_app::surface::{Action, AdminSection};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The staff console's sections this crate hands `oxy-app`, merged inside
/// `/api/admin` before the refusal while acting, each behind the gate
/// `admin::router` gave it in-tree:
///
/// - the report itself and the caller's own switch (`/usage-report`,
///   `/usage-report/email-preference`, `/usage-report/send-to-me`) behind
///   `PlatformOperate`, the capability [`usage_report::delivery::AUDIENCE`]
///   mails, so the mail's link always opens;
/// - who gets it, and switching it off for another person
///   (`/usage-report/recipients*`) behind `PlatformGrants`: the staff list,
///   so the capability that administers staff access;
/// - the Billing queue ([`admin::billing`]: `/orgs`, `/billing/prices`,
///   `/orgs/{org_id}/billing/*`) behind `PlatformOwnerOnly`, the action whose
///   ring (`Ring::GlobalOwnerOnly`) names the Billing queue. `oxy-app` gates a
///   section naming it with the strict owner guard (`oxy_owner_guard`) that
///   `admin::router` layered on billing in-tree, not the capability guard: a
///   Global Admin is 403 there and no database is read to say so.
///
/// Sections, not nests, because that is how they were mounted: a
/// `route_layer` on a router merged inside `admin::router`, which is exactly
/// what `extracted_sections` builds. None declares anything: the console's
/// `/{*rest}` FleetOk wildcard places them, as it did. The usage report's
/// handlers narrow a bounded grant to the orgs it reaches themselves
/// (`staff::scope`); the Billing queue's caller is the Global Owner, unbounded.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![
        AdminSection {
            capability: Action::PlatformOperate,
            routes: usage_report::router(),
            decls: Vec::new(),
        },
        AdminSection {
            capability: Action::PlatformGrants,
            routes: usage_report::recipients::router(),
            decls: Vec::new(),
        },
        AdminSection {
            capability: Action::PlatformOwnerOnly,
            routes: admin::billing::router(),
            decls: Vec::new(),
        },
    ]
}

/// The api seam's share: the tenant's billing routes under
/// `/orgs/{org_id}/billing`, merged at the protected root (`/api`) where
/// `build_global_routes` nested `build_org_routes`, so they keep the session
/// auth stack; cloud only, like the global tree they left.
///
/// Carries `org_middleware` and NOT the subscription guard, as
/// `build_org_routes` mounted them: billing is the one org-scoped tree a
/// paywalled org must still reach, to subscribe or open the portal.
pub fn routes() -> Router<AppState> {
    Router::new().nest("/orgs/{org_id}", billing::org_routes())
}

/// Pod placement for [`routes`], absolute (merged at the protected root, so
/// there is no prefix to join): the two rows `oxy-app`'s
/// `nest_all("/billing", RouteRole::FleetOk, ..)` declared, method `*`. The
/// Stripe customer and subscription rows, and Stripe itself.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    ROUTE_ROLES
}

const ROUTE_ROLES: &[RouteRoleDecl] = &[
    RouteRoleDecl {
        method: "*",
        path: "/orgs/{org_id}/billing/{*rest}",
        role: RouteRole::FleetOk,
    },
    RouteRoleDecl {
        method: "*",
        path: "/orgs/{org_id}/billing",
        role: RouteRole::FleetOk,
    },
];

/// The public seam's share: Stripe's webhook, beside `oxy-app`'s public tree,
/// outside the auth stack. Stripe carries no user credential; the handler
/// verifies the `stripe-signature` header over the raw body, and answers 404
/// while Stripe is not configured.
pub fn public_routes() -> Router<AppState> {
    Router::new().route("/webhooks/stripe", post(billing::webhook::stripe_webhook))
}

/// Pod placement for [`public_routes`], relative to `/api`: method `*`,
/// FleetOk, as `oxy-app`'s `route_fleet` declared it. It reads and writes the
/// webhook-event and billing rows in Postgres, so any replica serves it.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    &[RouteRoleDecl {
        method: "*",
        path: "/webhooks/stripe",
        role: RouteRole::FleetOk,
    }]
}

/// The weekly custom-app usage report's pass ([`usage_report::job`]), run by
/// the global driver once per iteration of its loop (`TickPass::EveryLoop`):
/// where `router::recovery` called `job::tick` inline before the move, after
/// the pass and the previews sweep, on every process that runs the driver
/// (an `all`/`ide` serve with `OXY_INPROC_GLOBAL_WORKER` on, each `oxy
/// worker`), in both modes and whatever that node claimed.
///
/// Not `TickPass::AfterSchedules`: in the cloud pass that runs only on a node
/// that claimed a workspace, and the report belongs to none — a deployment
/// with no workspace to claim would never write one. The job throttles itself
/// to once per ten minutes per process and claims its work by unique rows, so
/// every driver running it writes one report and sends one mail between them.
/// It reports nothing advanced: it logs its own outcome, as it did.
pub fn global_ticks() -> Vec<GlobalTickSeam> {
    vec![GlobalTickSeam {
        name: "usage_report",
        pass: TickPass::EveryLoop,
        tick: |db, _now| {
            Box::pin(async move {
                usage_report::job::tick(db).await;
                0
            })
        },
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// Every path the console calls, by the section that must mount it.
    const REPORT: &[&str] = &[
        "/usage-report",
        "/usage-report/email-preference",
        "/usage-report/send-to-me",
    ];
    const RECIPIENTS: &[&str] = &[
        "/usage-report/recipients",
        "/usage-report/recipients/someone@example.com",
    ];
    /// The Billing queue's paths (admin split PR 9), one per route.
    const BILLING_QUEUE: &[&str] = &[
        "/orgs",
        "/billing/prices",
        "/orgs/00000000-0000-0000-0000-000000000000/billing/subscription",
        "/orgs/00000000-0000-0000-0000-000000000000/billing/provision-subscription",
        "/orgs/00000000-0000-0000-0000-000000000000/billing/provision-checkout",
        "/orgs/00000000-0000-0000-0000-000000000000/billing/checkout",
        "/orgs/00000000-0000-0000-0000-000000000000/billing/checkout/resend",
        "/orgs/00000000-0000-0000-0000-000000000000/billing/checkout/cancel",
        "/orgs/00000000-0000-0000-0000-000000000000/billing/resync",
    ];

    /// TRACE, which no route takes: 405 from a mounted path, 404 from an
    /// unmounted one, without running a handler or reaching a database.
    async fn trace(router: &axum::Router, uri: &str) -> StatusCode {
        router
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
            .status()
    }

    /// The three sections `oxy-app`'s `admin::router` merged before the
    /// moves: the report behind `PlatformOperate`, its recipients behind
    /// `PlatformGrants` (PR 8b), and the Billing queue naming
    /// `PlatformOwnerOnly` (PR 9), which `oxy-app` gates with the strict owner
    /// guard `billing` carried in-tree. Each mounts its own paths and not the
    /// others', and none declares a placement. A swapped capability, or a
    /// route moved into another section, would hand the staff list to whoever
    /// reads the report (or the Billing queue to any staff member), and every
    /// route would still answer.
    #[tokio::test]
    async fn each_section_mounts_its_own_routes_behind_the_capability_it_had() {
        let sections = admin_sections();
        let got: Vec<_> = sections
            .iter()
            .map(|s| (s.capability, s.decls.len()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Action::PlatformOperate, 0),
                (Action::PlatformGrants, 0),
                (Action::PlatformOwnerOnly, 0)
            ]
        );
        let paths = [REPORT, RECIPIENTS, BILLING_QUEUE];
        for (i, section) in sections.iter().enumerate() {
            let router = section
                .routes
                .clone()
                .with_state(oxy_app::surface::bare_app_state());
            for (j, set) in paths.iter().enumerate() {
                let want = if i == j {
                    StatusCode::METHOD_NOT_ALLOWED
                } else {
                    StatusCode::NOT_FOUND
                };
                for path in *set {
                    assert_eq!(
                        trace(&router, path).await,
                        want,
                        "{:?} on {path}",
                        section.capability
                    );
                }
            }
            for path in [
                "/usage-report/",
                "/usage-reportx",
                "/usage-report/recipients/a/b",
                "/orgs/",
                "/billing",
                "/orgs/00000000-0000-0000-0000-000000000000/billing",
                "/orgs/00000000-0000-0000-0000-000000000000/billing/nothing",
            ] {
                assert_eq!(trace(&router, path).await, StatusCode::NOT_FOUND, "{path}");
            }
        }
    }

    /// The tenant's billing routes, relative to their `/orgs/{org_id}/billing`
    /// nest, as `oxy-app`'s `api::billing::router` mounted them: `/` is the
    /// summary, and nothing else answers.
    #[tokio::test]
    async fn the_tenant_router_mounts_the_five_billing_routes() {
        let router = Router::new()
            .nest("/billing", billing::router())
            .with_state(oxy_app::surface::bare_app_state());
        for path in [
            "/billing",
            "/billing/status",
            "/billing/portal-session",
            "/billing/invoices",
            "/billing/checkout-sessions/cs_test",
        ] {
            assert_eq!(
                trace(&router, path).await,
                StatusCode::METHOD_NOT_ALLOWED,
                "{path}"
            );
        }
        for path in [
            "/billing/nothing",
            "/billing/checkout-sessions",
            "/billingx",
        ] {
            assert_eq!(trace(&router, path).await, StatusCode::NOT_FOUND, "{path}");
        }
    }

    /// The api seam's share runs behind `org_middleware`, as
    /// `build_org_routes` mounted it: an org id the middleware's `Path` cannot
    /// parse is its 400, answered before any database is read, where the bare
    /// routes would answer TRACE 405. Nothing else under `/orgs/{org_id}` is
    /// this crate's.
    #[tokio::test]
    async fn the_tenant_routes_run_behind_org_middleware() {
        let router = routes().with_state(oxy_app::surface::bare_app_state());
        for path in [
            "/orgs/not-a-uuid/billing",
            "/orgs/not-a-uuid/billing/status",
            "/orgs/not-a-uuid/billing/portal-session",
            "/orgs/not-a-uuid/billing/invoices",
            "/orgs/not-a-uuid/billing/checkout-sessions/cs_test",
        ] {
            assert_eq!(
                trace(&router, path).await,
                StatusCode::BAD_REQUEST,
                "{path}"
            );
        }
        for path in [
            "/orgs/not-a-uuid",
            "/orgs/not-a-uuid/members",
            "/orgs/not-a-uuid/billingx",
        ] {
            assert_eq!(trace(&router, path).await, StatusCode::NOT_FOUND, "{path}");
        }
    }

    /// The two rows `oxy-app`'s `nest_all("/billing", RouteRole::FleetOk, ..)`
    /// declared, and the webhook's one `route_fleet` row: declared, not merely
    /// unclassified (which also reads FleetOk and is what a dropped row would
    /// look like), and matched by the classifier on live URIs.
    #[test]
    fn the_tenant_routes_and_the_webhook_are_declared_fleet_ok() {
        use oxy_app::surface::roles::declared_role_in;
        let api: Vec<_> = route_roles()
            .iter()
            .map(|d| (d.method, format!("/api{}", d.path), d.role))
            .collect();
        assert_eq!(api.len(), 2);
        for uri in [
            "/api/orgs/00000000-0000-0000-0000-000000000000/billing",
            "/api/orgs/00000000-0000-0000-0000-000000000000/billing/status",
            "/api/orgs/00000000-0000-0000-0000-000000000000/billing/checkout-sessions/cs",
        ] {
            assert_eq!(
                declared_role_in(&api, "GET", uri),
                Some(RouteRole::FleetOk),
                "{uri}"
            );
        }
        let public: Vec<_> = public_route_roles()
            .iter()
            .map(|d| (d.method, format!("/api{}", d.path), d.role))
            .collect();
        assert_eq!(public.len(), 1);
        assert_eq!(
            declared_role_in(&public, "POST", "/api/webhooks/stripe"),
            Some(RouteRole::FleetOk)
        );
    }

    /// The webhook has a route behind its declaration, and only one: TRACE,
    /// which it does not take, answers 405 there and 404 beside it, without
    /// running the handler (which reads the environment and a database).
    #[tokio::test]
    async fn the_public_router_mounts_only_the_stripe_webhook() {
        let router = public_routes().with_state(oxy_app::surface::bare_app_state());
        assert_eq!(
            trace(&router, "/webhooks/stripe").await,
            StatusCode::METHOD_NOT_ALLOWED
        );
        for path in ["/webhooks/stripe/", "/webhooks/github", "/billing"] {
            assert_eq!(trace(&router, path).await, StatusCode::NOT_FOUND, "{path}");
        }
    }

    /// The pass runs on every iteration of the driver's loop, as the inline
    /// call in `router::recovery` did, and not only on a node that claimed a
    /// workspace.
    #[test]
    fn the_weekly_pass_runs_on_every_loop_of_the_driver() {
        let ticks: Vec<_> = global_ticks().iter().map(|t| (t.name, t.pass)).collect();
        assert_eq!(ticks, vec![("usage_report", TickPass::EveryLoop)]);
    }
}
