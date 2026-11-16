//! Admin → Observability: what an org costs us, and how its apps perform.
//!
//! The staff console's half of this crate. The tenant handlers beside it
//! (`traces`, `metrics`, `execution_analytics`) answer a workspace's own
//! debugging questions; this answers the platform's two: **what is this org
//! costing us**, and **are its apps working well**.
//!
//! Design, including why there is no per-org infrastructure rate card:
//! `internal-docs/2026-11-16-admin-observability-design.md`.
//!
//! ## Three routes
//!
//! | | |
//! | --- | --- |
//! | `GET /observability/fleet` | one row per org — the list, so the surface answers "what is out there?" |
//! | `GET /observability/orgs/{org_id}/cost` | one org's LLM spend, five ways |
//! | `GET /observability/orgs/{org_id}/apps` | one org's custom-app performance |
//! | `GET /observability/orgs/{org_id}/resources` | what the org consumes, by meter |
//!
//! Beside them, Admin → Metrics' LLM cost ([`metrics`]: `GET /metrics/llm-usage`,
//! the cross-tenant overview, and `GET /metrics/orgs/{org_id}/llm-usage`, one
//! org's daily series), moved out of `oxy-app`'s `admin::metrics` in the admin
//! split behind the same capability. Its SQL is [`metrics_rollup`] and its
//! pricing [`metrics_pricing`], both on `oxy_app_core::llm_usage`, the rate
//! table `cost_rollup` prices with.
//!
//! ## Two rules every handler here keeps
//!
//! **The staff-scope fence runs before the read.** `PlatformOperate` is held
//! by every Global Admin *regardless of bound*, so an unfenced read answers
//! another tenant's spend to an org-scoped App Operator. The fence is
//! `oxy_app::surface::admin_scope`'s, never re-decided here — the one rule
//! authz has. An out-of-scope org answers **not found**, which is what a scope
//! boundary looks like throughout this console.
//!
//! **"Not measured" is a third outcome.** Cost comes from Postgres and is
//! always answerable. App performance comes from the ClickHouse product store,
//! which is off unless `OXY_OBSERVABILITY_BACKEND` is set — so the responses
//! carry `measured` / `apps_measured`, and the page names an unmeasured meter
//! once rather than drawing a zero or a dash per row.

mod apps;
mod cost;
mod cost_rollup;
mod derive;
mod fleet;
mod meters;
pub mod metrics;
mod metrics_pricing;
mod metrics_rollup;
mod rates;
mod resources;
mod scale;
mod storage_rollup;

use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use oxy_app::surface::admin_db::{connect, error_body};
use oxy_app::surface::{Action, AdminSection};
use oxy_app_core::AppState;
use oxy_auth::AuthenticatedUser;
use oxy_shared::errors::OxyError;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};
use serde::Deserialize;
use uuid::Uuid;

/// The capability that admits a caller.
///
/// `PlatformOperate`, the same gate the other fleet-wide readouts take — Usage
/// report, Workspace health, Internal jobs. Deliberately **not** `PlatformApps`:
/// an App Operator ships custom apps and nothing else, and what every tenant
/// costs the company is not part of shipping one.
pub const CAPABILITY: Action = Action::PlatformOperate;

/// This surface's staff-console section, for `oxy-server` to merge.
///
/// `oxy-app` applies the capability guard with `route_layer`, so a route added
/// to [`routes`] is behind it automatically — a section cannot mount an admin
/// route that skips its own gate.
pub fn admin_sections() -> Vec<AdminSection> {
    vec![AdminSection {
        capability: CAPABILITY,
        routes: routes(),
        decls: route_roles().to_vec(),
    }]
}

/// Relative to `/admin`. Its own function so the route catalog can seed it.
pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(fleet::router())
        .merge(cost::router())
        .merge(apps::router())
        .merge(resources::router())
        .merge(metrics::router())
}

/// Pod placement, relative to `/admin`.
///
/// Every route is `FleetOk`: all are reads of persisted data — Postgres for
/// cost and LLM usage, the ClickHouse product store for performance — and no handler
/// touches the working copy, `.git` or the state directory. Every replica
/// reaches both the same way, or none does.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    ROUTE_ROLES
}

const fn fleet_ok(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// A `const`, not a literal in the function: `fleet_ok` is a `const fn`, and a
/// slice of its calls is promoted to `'static` only through a constant.
const ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet_ok("/observability/fleet"),
    fleet_ok("/observability/orgs/{org_id}/cost"),
    fleet_ok("/observability/orgs/{org_id}/apps"),
    fleet_ok("/observability/orgs/{org_id}/resources"),
    fleet_ok("/metrics/llm-usage"),
    fleet_ok("/metrics/orgs/{org_id}/llm-usage"),
];

/// The window every route takes.
#[derive(Debug, Default, Deserialize)]
pub(super) struct WindowQuery {
    days: Option<i32>,
}

impl WindowQuery {
    /// The window in days: 30 by default, clamped to 1..=365.
    ///
    /// 30 because the question behind a cost page is "what will this cost us",
    /// and a single day is noise — the same default and the same reason as
    /// Vercel's usage page. Clamped because `days` is a request parameter and
    /// it reaches `make_interval` and a ClickHouse `INTERVAL`: a negative
    /// window would silently select nothing, and an unbounded one would scan
    /// every partition of a 90-day table.
    pub(super) fn days(&self) -> i32 {
        self.days.unwrap_or(30).clamp(1, 365)
    }
}

/// A connection, with this org already checked against the caller's grant.
///
/// The fence first, then the read — not the other way around, and never a
/// hand-rolled role check. An out-of-scope org answers not found.
pub(super) async fn connect_scoped(
    actor: &AuthenticatedUser,
    org_id: Uuid,
) -> Result<sea_orm::DatabaseConnection, Response> {
    let db = connect().await?;
    oxy_app::surface::admin_scope::deny_out_of_scope(&db, actor, org_id)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok(db)
}

/// A connection and the caller's org fence for a cross-tenant read.
///
/// `None` is an unbounded reader. `Some(orgs)` is a bounded grant, and
/// `Some(&[])` is a grant that reaches nothing — which the caller must
/// short-circuit rather than pass to an `IN ()`.
pub(super) async fn connect_fleet(
    actor: &AuthenticatedUser,
) -> Result<(sea_orm::DatabaseConnection, Option<Vec<Uuid>>), Response> {
    let db = connect().await?;
    let scope = oxy_app::surface::admin_scope::list_scope(&db, actor)
        .await
        .map_err(IntoResponse::into_response)?;
    Ok((db, scope))
}

/// An observability-store failure, as this console reports one.
///
/// 503 rather than 500: the store being unreachable is a dependency being
/// down, and the page's retry is the right response to it. The store's own
/// error text is passed through — with no backend configured it names the env
/// vars that enable one, which is the actionable half of the message.
pub(super) fn store_err(e: OxyError) -> Response {
    tracing::warn!("admin observability store read failed: {e}");
    error_body(
        StatusCode::SERVICE_UNAVAILABLE,
        "observability_unavailable",
        Some(e.to_string()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request};
    use tower::ServiceExt;

    /// Every concrete route the section mounts, path parameters filled.
    const MOUNTED: &[&str] = &[
        "/observability/fleet",
        "/observability/orgs/d9830be4-c6a4-4d4e-9b0e-6f1f0f1a2b3c/cost",
        "/observability/orgs/d9830be4-c6a4-4d4e-9b0e-6f1f0f1a2b3c/apps",
        "/observability/orgs/d9830be4-c6a4-4d4e-9b0e-6f1f0f1a2b3c/resources",
        "/metrics/llm-usage",
        "/metrics/orgs/d9830be4-c6a4-4d4e-9b0e-6f1f0f1a2b3c/llm-usage",
    ];

    async fn trace(router: &Router, uri: &str) -> StatusCode {
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

    /// A declaration with no route behind it — or a route moved without its
    /// declaration — leaves the two lists disagreeing, and the symptom is a
    /// route served from a pod that cannot answer it. TRACE, which no route
    /// here takes, answers 405 from a mounted path and 404 from an unmounted
    /// one, without running a handler.
    #[tokio::test]
    async fn every_route_is_mounted() {
        let router = routes().with_state(oxy_app::surface::bare_app_state());
        for uri in MOUNTED {
            assert_eq!(
                trace(&router, uri).await,
                StatusCode::METHOD_NOT_ALLOWED,
                "{uri}: not mounted"
            );
        }
        for uri in ["/observability", "/observability/orgs", "/nothing-here"] {
            assert_eq!(
                trace(&router, uri).await,
                StatusCode::NOT_FOUND,
                "{uri} must stay unmounted"
            );
        }
    }

    /// Each row is DECLARED `FleetOk` exactly once — not merely unclassified,
    /// which `classify` also answers `FleetOk` and is what a dropped row would
    /// look like. The declaration is the point; a missing one is invisible.
    #[test]
    fn every_route_is_declared_fleet_ok_exactly_once() {
        let declared = route_roles();
        assert_eq!(declared.len(), MOUNTED.len(), "one row per mounted route");
        for decl in declared {
            let same = declared.iter().filter(|d| d.path == decl.path).count();
            assert_eq!(same, 1, "{}: declared {same} times", decl.path);
            assert_eq!(decl.role, RouteRole::FleetOk, "{}", decl.path);
            assert_eq!(decl.method, "*", "{}", decl.path);
        }
    }

    /// The declarations hold against the classifier the way the served router
    /// uses them: prefixed by `/api/admin`, then matched against a live URI.
    #[test]
    fn every_mounted_route_classifies_fleet_ok() {
        use oxy_app::surface::roles::declared_role_in;
        let decls: Vec<_> = route_roles()
            .iter()
            .map(|d| (d.method, format!("/api/admin{}", d.path), d.role))
            .collect();
        for path in MOUNTED {
            let uri = format!("/api/admin{path}");
            assert_eq!(
                declared_role_in(&decls, "GET", &uri),
                Some(RouteRole::FleetOk),
                "GET {uri} must match a FleetOk declaration"
            );
        }
    }

    /// The section names `PlatformOperate`, the gate the other fleet-wide
    /// readouts take. Pinned because the capability is the whole access
    /// decision for this surface: widening it to `PlatformApps` would hand
    /// every tenant's spend to an App Operator, who holds nothing else.
    #[test]
    fn the_section_is_gated_on_platform_operate() {
        let sections = admin_sections();
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].capability, Action::PlatformOperate);
    }

    /// The window defaults to 30 days and is clamped. `days` reaches both
    /// `make_interval(days => $1)` and a ClickHouse `INTERVAL`: a negative
    /// value selects nothing (a page of zeros that reads as a quiet platform),
    /// and an unbounded one scans every partition of a 90-day table.
    #[test]
    fn the_window_defaults_to_thirty_days_and_is_clamped() {
        assert_eq!(WindowQuery { days: None }.days(), 30);
        assert_eq!(WindowQuery { days: Some(7) }.days(), 7);
        assert_eq!(WindowQuery { days: Some(0) }.days(), 1);
        assert_eq!(WindowQuery { days: Some(-9_000) }.days(), 1);
        assert_eq!(
            WindowQuery {
                days: Some(100_000)
            }
            .days(),
            365
        );
    }
}
