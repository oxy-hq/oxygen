//! The workspace's own configured resources, out of `oxy-app` (Pending 8 in
//! `internal-docs/domain-boundaries.md`): Workspace Settings' Databases and
//! Secrets pages, and the database reads the IDE and the launcher make.
//!
//! - [`database`]: the configured databases (`config.yml`'s `databases:` and
//!   their synced datasets), a database's live schema, and the connection
//!   probe and schema inspection the onboarding flow streams.
//! - [`sync`] (2026-11-21): `POST /databases/sync`, one database's
//!   `.databases/` files rendered from the warehouse for `oxyc sync` to write
//!   into a checkout (it was `oxy sync`). Writes no server state.
//! - [`secrets`]: the workspace's project secrets (`secrets`, a child of the
//!   Workspace aggregate), CRUD, bulk create and reveal. A custom app's
//!   non-production secret is staff's and never listed or reached here; a
//!   production app secret's write is audited (`oxy_custom_apps::secret_audit`).
//! - [`integration`] (2026-11-16): the Looker integrations `config.yml`
//!   declares, with the explores each names, read from the served revision's
//!   compiled config (`/integrations/looker`), and (2026-11-21) the credentials
//!   check for one (`/integrations/looker/{name}/test`, `WorkspaceAdmin`), which
//!   replaced the Rust `oxy looker test`.
//! - [`integrations`] (2026-11-21, out of `oxy-app`'s `integrations`): the
//!   OAuth "Connect" flow that lands a vendor's refresh token in this
//!   workspace's secrets — QuickBooks' legacy authorize and callback, and the
//!   uniform `/integrations/oauth/{provider}/authorize` and
//!   `/oauth/{provider}/callback` over the provider table.
//!
//! [`workspace_routes`] rides the workspace seam, merged INSIDE the
//! `/api/{workspace_id}` nest, so it inherits the session auth stack and
//! `workspace_middleware`, where `build_workspace_routes` mounted
//! `/databases`, `/integrations` and `/secrets` (the latter through
//! `router::secrets`). Every mutation takes `WorkspaceAdmin` in its signature,
//! so no route-level middleware is needed, as before.
//!
//! [`public_routes`] rides the public seam, merged beside `oxy-app`'s public
//! tree OUTSIDE the auth stack, in both modes, where `build_public_routes`
//! mounted the two OAuth callbacks: the vendor redirects a bare browser there
//! with no Oxy session, and the single-use state nonce is the guard.
//!
//! Every route is FleetOk, method `*`, as `route_fleet` declared them: the
//! listing reads the in-memory `Config` (its datasets degrade to `null` without
//! a working copy), the schema, inspection and sync talk to the warehouse, the
//! secrets live in Postgres, and the OAuth connects write a Postgres state row
//! and the workspace's secrets, never a working copy or the local state dir.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

pub mod database;
pub mod integration;
pub mod integrations;
pub mod secrets;
pub mod sync;

use axum::Router;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The workspace seam's share: merged inside `/api/{workspace_id}`. The two
/// nests are as `oxy-app` nested them, so each prefix answers exactly what it
/// did (the list is its `/`). `/integrations/*` are single routes, not a nest:
/// the Looker listing (named in the served revision's config), the Looker
/// credentials check beside it (`/{name}/test`, `oxyc looker test`), and the
/// two OAuth connects, which `oxy-app`'s `build_integration_routes` nested
/// under `/integrations` until 2026-11-21. The check and both connects take
/// `WorkspaceAdmin` in their signatures (see
/// [`integrations::quickbooks::oauth::authorize`]).
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
        .merge(integration_connect_routes())
}

/// The OAuth connects, as `build_integration_routes` mounted them. The handler
/// paths are spelled as they were in `oxy-app` (`crate::integrations::…`, the
/// same module path in this crate), which keeps the route catalog's `handler`
/// field byte-identical.
fn integration_connect_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/integrations/quickbooks/authorize",
            post(crate::integrations::quickbooks::oauth::authorize::authorize),
        )
        .route(
            "/integrations/oauth/{provider}/authorize",
            post(crate::integrations::quickbooks::oauth::authorize::authorize_by_slug),
        )
}

/// The public seam's share: merged beside `oxy-app`'s public tree, outside
/// the auth stack, in both modes, where `build_public_routes` mounted them
/// until 2026-11-21. Public because Intuit (or any provider) redirects the
/// bare browser here after consent; the state nonce (single-use, 15-min TTL,
/// matched on provider) is the CSRF/replay guard. QuickBooks keeps its own
/// path because that URL is registered in every customer's Intuit app and can
/// never be renamed; every other provider takes the uniform one.
pub fn public_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/quickbooks/oauth/callback",
            get(crate::integrations::quickbooks::oauth::callback::callback),
        )
        .route(
            "/oauth/{provider}/callback",
            get(crate::integrations::quickbooks::oauth::callback::callback_by_slug),
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
        // One database's `.databases/` files, rendered from the compiled
        // config and the warehouse; `oxyc sync` writes them locally.
        .route("/sync", post(sync::sync_database_handler))
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
/// the twelve rows `oxy-app`'s `route_fleet` declared, method `*`, FleetOk, plus
/// the Looker credentials check and `/databases/sync` added here (2026-11-21).
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

/// Pod placement for [`public_routes`], relative to the public tree (the
/// served router prefixes `/api`): the two rows `build_public_routes`'
/// `route_fleet` declared, method `*`, FleetOk. The callback exchanges the
/// code with the vendor and writes the workspace's secrets and the state row,
/// all Postgres.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    PUBLIC_ROUTE_ROLES
}

const PUBLIC_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/quickbooks/oauth/callback"),
    fleet("/oauth/{provider}/callback"),
];

/// A `const`, not a literal in the function: `fleet` is a `const fn`, and a
/// slice of its calls is promoted to `'static` only through a constant.
const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/databases"),
    fleet("/databases/inspect"),
    fleet("/databases/inspect-schemas"),
    fleet("/databases/inspect-schema-tables"),
    fleet("/databases/{database_name}/schema"),
    // Renders from the compiled config and the warehouse; writes nothing
    // (2026-11-21, `oxyc sync`).
    fleet("/databases/sync"),
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
    // The OAuth connects (2026-11-21, out of `build_integration_routes`): a
    // Postgres state row and the workspace secret manager, no filesystem and
    // no local state dir.
    fleet("/integrations/quickbooks/authorize"),
    fleet("/integrations/oauth/{provider}/authorize"),
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
        "/databases/sync",
        "/secrets",
        "/secrets/bulk",
        "/secrets/7c9e6679-7425-40de-944b-e07fc1f90ae7",
        "/secrets/7c9e6679-7425-40de-944b-e07fc1f90ae7/value",
        "/integrations/looker",
        "/integrations/looker/my_looker/test",
        "/integrations/quickbooks/authorize",
        "/integrations/oauth/google/authorize",
    ];

    /// Every concrete public route, with path parameters filled.
    const PUBLIC_MOUNTED: &[&str] = &["/quickbooks/oauth/callback", "/oauth/google/callback"];

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
            14,
            "six database paths, four secret paths, four integration paths"
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
        use oxy_app_core::pods::role_manifest::declared_role_in;
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
            ("POST", "/databases/sync"),
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
            // The rest of that ack list, moved with the OAuth connects on
            // 2026-11-21 (the test went with them: `build_integration_routes`
            // no longer exists). Each writes a Postgres state row and upserts
            // the client secret through the workspace secret manager. No
            // working copy, no .git, no local state dir — so any replica can
            // serve it.
            ("POST", "/integrations/quickbooks/authorize"),
            ("POST", "/integrations/oauth/google/authorize"),
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
        use oxy_server_authz::workspace_role::EffectiveWorkspaceRole;

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

    /// The public seam's two rows, as `build_public_routes`' `route_fleet`
    /// declared them: method `*`, FleetOk, each exactly once.
    #[test]
    fn every_public_route_is_declared_fleet_ok_exactly_once() {
        let declared = public_route_roles();
        assert_eq!(declared.len(), 2, "the two OAuth callbacks");
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

    /// Classified against this crate's declarations, `/api`-prefixed as the
    /// served router installs the public seam's, so a dropped row fails rather
    /// than falling to `classify`'s FleetOk default. The callbacks exchange the
    /// code with the vendor and write the workspace's secrets: Postgres only.
    #[test]
    fn every_public_route_classifies_fleet_ok() {
        use oxy_app_core::pods::role_manifest::declared_role_in;
        let decls: Vec<_> = public_route_roles()
            .iter()
            .map(|d| (d.method, format!("/api{}", d.path), d.role))
            .collect();
        for path in PUBLIC_MOUNTED {
            let uri = format!("/api{path}");
            assert_eq!(
                declared_role_in(&decls, "GET", &uri),
                Some(RouteRole::FleetOk),
                "GET {uri} must match a FleetOk declaration"
            );
        }
    }

    /// The public declarations and routes agree, by the same TRACE probe as
    /// [`every_route_is_mounted`]: 405 from a mounted (GET-only) path, 404
    /// from an unmounted one, without running a handler.
    #[tokio::test]
    async fn every_public_route_is_mounted() {
        let router = public_routes().with_state(oxy_app::surface::bare_app_state());
        for uri in PUBLIC_MOUNTED {
            assert_eq!(
                trace(&router, uri).await,
                StatusCode::METHOD_NOT_ALLOWED,
                "{uri}: not mounted"
            );
        }
        assert_eq!(
            trace(&router, "/oauth/google/nothing-mounts-this").await,
            StatusCode::NOT_FOUND,
            "control"
        );
    }
}
