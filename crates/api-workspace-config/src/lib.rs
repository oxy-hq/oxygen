//! The workspace's own configured resources, out of `oxy-app` (Pending 8 in
//! `internal-docs/domain-boundaries.md`): Workspace Settings' Databases and
//! Secrets pages, and the database reads the IDE and the launcher make.
//!
//! - [`database`]: the configured databases (`config.yml`'s `databases:` and
//!   their synced datasets), a database's live schema, and the connection
//!   probe and schema inspection the onboarding flow streams.
//! - [`secrets`]: the workspace's project secrets (`secrets`, a child of the
//!   Workspace aggregate), CRUD, bulk create and reveal. A custom app's
//!   non-production secret is staff's and never listed or reached here; a
//!   production app secret's write is audited (`oxy_custom_apps::secret_audit`).
//! - [`integration`] (2026-11-16): the Looker integrations `config.yml`
//!   declares, with the explores each names, read from the served revision's
//!   compiled config (`/integrations/looker`), and (2026-11-21) the credentials
//!   check for one (`/integrations/looker/{name}/test`, `WorkspaceAdmin`), which
//!   replaced the Rust `oxy looker test`. The other `/integrations/*` routes
//!   (the OAuth connects) are still `oxy-app`'s.
//!
//! [`workspace_routes`] rides the workspace seam, merged INSIDE the
//! `/api/{workspace_id}` nest, so it inherits the session auth stack and
//! `workspace_middleware`, where `build_workspace_routes` mounted
//! `/databases` and `/secrets` (the latter through `router::secrets`). Every
//! mutation takes `WorkspaceAdmin` in its signature, so no route-level
//! middleware is needed, as before.
//!
//! Every route is FleetOk, method `*`, as `route_fleet` declared them: the
//! listing reads the in-memory `Config` (its datasets degrade to `null` without
//! a working copy), the schema and inspection talk to the warehouse, and the
//! secrets live in Postgres.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

pub mod database;
pub mod integration;
pub mod secrets;

use axum::Router;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The workspace seam's share: merged inside `/api/{workspace_id}`. The two
/// nests are as `oxy-app` nested them, so each prefix answers exactly what it
/// did (the list is its `/`). `/integrations/looker` is a single route, not a
/// nest: `oxy-app` still nests the rest of `/integrations` (the OAuth
/// connects), and the two merge as siblings under that prefix. It lists the
/// Looker integrations named in the served revision's config; `/{name}/test`
/// beside it checks one integration's credentials (`oxyc looker test`).
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .nest("/databases", database_routes())
        .nest("/secrets", secret_routes())
        .route(
            "/integrations/looker",
            get(integration::list_looker_integrations),
        )
        .route(
            "/integrations/looker/{name}/test",
            post(integration::test_looker_integration),
        )
}

fn database_routes() -> Router<AppState> {
    Router::new()
        // The list degrades to `datasets: null` without a working copy, and the
        // launcher readiness check calls it on every page load.
        .route("/", get(database::list_databases))
        .route("/inspect", post(database::inspect_database_handler))
        .route("/inspect-schemas", post(database::inspect_schemas_handler))
        .route(
            "/inspect-schema-tables",
            post(database::inspect_schema_tables_handler),
        )
        .route(
            "/{database_name}/schema",
            get(database::get_database_schema),
        )
}

/// Each handler enforces `WorkspaceAdmin` via its extractor signature.
fn secret_routes() -> Router<AppState> {
    Router::new()
        .route("/", get(secrets::list_secrets).post(secrets::create_secret))
        .route("/bulk", post(secrets::bulk_create_secrets))
        .route(
            "/{id}",
            get(secrets::get_secret)
                .put(secrets::update_secret)
                .delete(secrets::delete_secret),
        )
        .route("/{id}/value", get(secrets::reveal_secret))
}

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree:
/// the ten rows `oxy-app`'s `route_fleet` declared, method `*`, FleetOk, and
/// the Looker credentials check added here (2026-11-21).
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

/// A `const`, not a literal in the function: `fleet` is a `const fn`, and a
/// slice of its calls is promoted to `'static` only through a constant.
const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/databases"),
    fleet("/databases/inspect"),
    fleet("/databases/inspect-schemas"),
    fleet("/databases/inspect-schema-tables"),
    fleet("/databases/{database_name}/schema"),
    fleet("/secrets"),
    fleet("/secrets/bulk"),
    fleet("/secrets/{id}"),
    fleet("/secrets/{id}/value"),
    // Reads the served revision's compiled config, never a working copy or
    // Looker itself (2026-11-16, out of `build_integration_routes`).
    fleet("/integrations/looker"),
    // Reads the compiled config and the secrets store, then calls Looker; no
    // working copy (2026-11-21, `oxyc looker test`).
    fleet("/integrations/looker/{name}/test"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    /// Every concrete route, with path parameters filled.
    const MOUNTED: &[&str] = &[
        "/databases",
        "/databases/inspect",
        "/databases/inspect-schemas",
        "/databases/inspect-schema-tables",
        "/databases/warehouse/schema",
        "/secrets",
        "/secrets/bulk",
        "/secrets/7c9e6679-7425-40de-944b-e07fc1f90ae7",
        "/secrets/7c9e6679-7425-40de-944b-e07fc1f90ae7/value",
        "/integrations/looker",
        "/integrations/looker/my_looker/test",
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
        assert_eq!(
            declared.len(),
            11,
            "five database paths, four secret paths, two integration paths"
        );
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

    /// Moved from `oxy-app`'s `role_manifest_tests::every_database_route_is_fleet_ok`
    /// (and the Looker row from `every_integration_sub_route_is_classified`),
    /// which would pass on the FleetOk default once the declarations left: the
    /// list degrades without a working copy, and the rest introspect the
    /// warehouse, not the disk. Classified against this crate's declarations,
    /// as the served router installs them, so a dropped row fails.
    #[test]
    fn every_database_secret_and_integration_route_classifies_fleet_ok() {
        use oxy_app::surface::roles::declared_role_in;
        let decls: Vec<_> = workspace_route_roles()
            .iter()
            .map(|d| (d.method, format!("/api/{{workspace_id}}{}", d.path), d.role))
            .collect();
        let ws = "d9830be4-c6a4";
        for (method, path) in [
            ("GET", "/databases"),
            ("POST", "/databases/inspect"),
            ("POST", "/databases/inspect-schemas"),
            ("POST", "/databases/inspect-schema-tables"),
            ("GET", "/databases/warehouse/schema"),
            ("GET", "/secrets"),
            ("POST", "/secrets"),
            ("POST", "/secrets/bulk"),
            ("PUT", "/secrets/7c9e6679"),
            ("DELETE", "/secrets/7c9e6679"),
            ("GET", "/secrets/7c9e6679/value"),
            // Moved with its route from `oxy-app`'s `role_manifest_tests::
            // every_integration_sub_route_is_classified`, whose ack list named
            // it: the listing reads the served revision's config.
            ("GET", "/integrations/looker"),
            // The credentials check: compiled config + secrets store + Looker.
            ("POST", "/integrations/looker/my_looker/test"),
        ] {
            let uri = format!("/api/{ws}{path}");
            assert_eq!(
                declared_role_in(&decls, method, &uri),
                Some(RouteRole::FleetOk),
                "{method} {uri} must match a FleetOk declaration"
            );
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

    /// The Looker credentials check takes `WorkspaceAdmin` FIRST, as the OAuth
    /// connects beside it do: a member or viewer is refused before the handler
    /// reads the config, so a 404 can never confirm an integration's name to
    /// them. No workspace manager is installed here, so reaching any later
    /// extractor would answer something other than 403.
    #[tokio::test]
    async fn the_looker_check_refuses_a_non_admin_before_anything_else() {
        use axum::extract::Request;
        use axum::middleware::{self, Next};
        use entity::workspace_members::WorkspaceRole;
        use oxy_app::surface::EffectiveWorkspaceRole;

        for role in [WorkspaceRole::Member, WorkspaceRole::Viewer] {
            let injected = role.clone();
            let inject = move |mut req: Request, next: Next| {
                let role = injected.clone();
                async move {
                    req.extensions_mut().insert(EffectiveWorkspaceRole(role));
                    next.run(req).await
                }
            };
            let router = Router::new()
                .nest("/{workspace_id}", workspace_routes())
                .layer(middleware::from_fn(inject))
                .with_state(oxy_app::surface::bare_app_state());
            let status = router
                .oneshot(
                    Request::builder()
                        .method(Method::POST)
                        .uri("/7c9e6679-7425-40de-944b-e07fc1f90ae7/integrations/looker/my_looker/test")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap()
                .status();
            assert_eq!(status, StatusCode::FORBIDDEN, "{role:?}");
        }
    }
}
