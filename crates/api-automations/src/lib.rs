//! The automations surface, out of `oxy-app` (Pending 8 in
//! `internal-docs/domain-boundaries.md`): what the IDE's and the customer nav's
//! Automations pages call that is not the execution surface itself.
//!
//! - [`automation`]: the automation list and one automation's YAML, served
//!   from the compile boundary (`procedure_definitions`) so the sidebar and the
//!   diagram render on a stateless replica. `/procedures` and its canonical
//!   alias `/automations`.
//! - [`schedules`]: cron schedule CRUD, run-now and backfill
//!   (`/agentic-schedules`), over `agentic_pipeline::scheduler`. A schedule
//!   fires an automation, an airway pipeline, an agent, a monitor scan, a
//!   health eval, a pre-aggregation cycle or a custom app's function; the
//!   page that lists them is the automations one.
//!
//! The execution surface (`/agentic-workflows`, `/agentic-automations`) is
//! `agentic-http`'s `automation_router`, still mounted by `oxy-app`. The
//! schedule handlers left `agentic-http` for `oxy-app` because they take
//! `WorkspaceAdmin`; they come here for the same reason, through the prelude.
//!
//! [`workspace_routes`] rides the workspace seam, merged INSIDE the
//! `/api/{workspace_id}` nest, so it inherits the session auth stack and
//! `workspace_middleware` (which inserts the `PlatformContext` run-now takes),
//! where `build_workspace_routes` mounted these. The schedule handlers extract
//! `Extension<Arc<AgenticState>>`; `oxy-app` layers it over every workspace
//! seam route (`protected::with_agentic_state`), where their own builder
//! layered it before.
//!
//! Every route is FleetOk, method `*`, as `route_fleet` declared them: the
//! listing reads the compile boundary (working copy only where the node holds
//! one, through `WorkspaceManagerReadOnly`), and the schedules read and write
//! Postgres.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

pub mod automation;
pub mod schedules;

use axum::Router;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The workspace seam's share: merged inside `/api/{workspace_id}`.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        // From the compile boundary, so the customer-nav sidebar renders on a
        // stateless serve replica with no working copy (`/agentic-workflows`
        // is IdeOnly). `/procedures` is kept for backward compatibility;
        // `/automations` is the canonical alias, same handlers.
        .route("/procedures", get(automation::list_automations))
        .route("/procedures/{path_b64}", get(automation::get_automation))
        .route("/automations", get(automation::list_automations))
        .route("/automations/{path_b64}", get(automation::get_automation))
        // Nested, as `oxy-app` nested it, so the prefix answers exactly what
        // it did (the list and create are its `/`).
        .nest("/agentic-schedules", schedule_routes())
}

fn schedule_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(schedules::list).post(schedules::create))
        .route(
            "/{id}",
            get(schedules::get)
                .patch(schedules::update)
                .delete(schedules::delete),
        )
        .route("/{id}/run-now", post(schedules::run_now))
        .route("/{id}/backfill", post(schedules::backfill))
}

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree:
/// the eight rows `oxy-app`'s `route_fleet` declared, method `*`, FleetOk.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

/// A `const`, not a literal in the function: `fleet` is a `const fn`, and a
/// slice of its calls is promoted to `'static` only through a constant.
const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/procedures"),
    fleet("/procedures/{path_b64}"),
    fleet("/automations"),
    fleet("/automations/{path_b64}"),
    fleet("/agentic-schedules"),
    fleet("/agentic-schedules/{id}"),
    fleet("/agentic-schedules/{id}/run-now"),
    fleet("/agentic-schedules/{id}/backfill"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// Every concrete route, with path parameters filled.
    const MOUNTED: &[&str] = &[
        "/procedures",
        "/procedures/YXV0b21hdGlvbnMvbmlnaHRseS5hdXRvbWF0aW9uLnltbA",
        "/automations",
        "/automations/YXV0b21hdGlvbnMvbmlnaHRseS5hdXRvbWF0aW9uLnltbA",
        "/agentic-schedules",
        "/agentic-schedules/65c121b2-0000-0000-0000-000000000000",
        "/agentic-schedules/65c121b2-0000-0000-0000-000000000000/run-now",
        "/agentic-schedules/65c121b2-0000-0000-0000-000000000000/backfill",
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

    /// The routes came from `oxy-app`, where `route_fleet` wrote these rows.
    /// Here the role is a declaration, so hold it: each row is DECLARED FleetOk
    /// exactly once — not merely unclassified, which `classify` also answers
    /// FleetOk and is what a dropped row would look like.
    #[test]
    fn every_route_is_declared_fleet_ok_exactly_once() {
        let declared = workspace_route_roles();
        assert_eq!(declared.len(), 8, "four listing paths, four schedule paths");
        for decl in declared {
            let same = declared
                .iter()
                .filter(|d| d.method == decl.method && d.path == decl.path)
                .count();
            assert_eq!(
                same, 1,
                "{} {}: declared {same} times",
                decl.method, decl.path
            );
            assert_eq!(decl.role, RouteRole::FleetOk, "{}", decl.path);
            assert_eq!(decl.method, "*", "{}", decl.path);
        }
    }

    /// The declarations hold against the role classifier the way the served
    /// router uses them: prefixed by the workspace tree, then matched against a
    /// live URI. Every mounted route must match a declaration, not the default.
    #[test]
    fn every_mounted_route_classifies_fleet_ok() {
        use oxy_app_core::pods::role_manifest::declared_role_in;
        let decls: Vec<_> = workspace_route_roles()
            .iter()
            .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
            .collect();
        for path in MOUNTED {
            let uri = format!("/api/d9830be4-c6a4{path}");
            for method in ["GET", "POST", "PATCH", "DELETE"] {
                assert_eq!(
                    declared_role_in(&decls, method, &uri),
                    Some(RouteRole::FleetOk),
                    "{method} {uri} must match a FleetOk declaration"
                );
            }
        }
    }

    /// A declaration with no route behind it (or a route moved without its
    /// declaration) leaves the two lists disagreeing. TRACE, which no route
    /// here takes, answers 405 from a mounted path and 404 from an unmounted
    /// one, without running a handler.
    #[tokio::test]
    async fn every_route_is_mounted() {
        let router = workspace_routes().with_state(oxy_app::surface::bare_app_state());
        for uri in MOUNTED {
            assert_eq!(
                trace(&router, uri).await,
                StatusCode::METHOD_NOT_ALLOWED,
                "{uri}: not mounted"
            );
        }
        assert_eq!(
            trace(&router, "/nothing-mounts-this").await,
            StatusCode::NOT_FOUND,
            "control"
        );
    }
}
