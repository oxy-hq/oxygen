//! Where this crate's routes mount, and which pod may serve them.
//!
//! Two halves, both merged at the protected-tree root by `oxy-server` through
//! the `extra_api_routes` seam — the same place `build_global_routes` sat when
//! these lived in `oxy-app`, so they inherit the standard auth stack from
//! `apply_middleware` rather than reproducing it.

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{get, patch, post};
use oxy_app::server::api::middlewares::org_context::org_middleware;
use oxy_app::server::api::middlewares::subscription_guard::subscription_guard_middleware;
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

use crate::{ask, ask_sessions, categories, handlers, manage, review, search, shelf, versions};

/// Every document route: the read side at the root, the manage side under
/// `/orgs/{org_id}`.
pub fn routes() -> Router<AppState> {
    Router::new()
        .merge(read_routes())
        .nest("/orgs/{org_id}", org_routes())
}

/// The routes in [`routes`] that may not take the FleetOk default.
///
/// Stated here because `oxy-app` owns `RoleRouter` and cannot see these
/// handlers — the same reason `oxy-api-onboarding` carries `route_roles()`.
/// Paths are absolute: `oxy-server` merges this crate at the protected-tree
/// root, so there is no prefix to join.
///
/// One entry, and the rest is deliberate: every other route here is Postgres
/// for the rows and a presigned object-store URL for the bytes, so it stays
/// FleetOk by default — reading the sanitiser SOP has to survive a deploy.
/// `ask` resolves an agent config out of the workspace working copy, which only
/// the ide singleton holds. `"*"` as `route_ide` declared it inside `oxy-app`.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &[RouteRoleDecl {
        method: "*",
        path: "/documents/ask",
        role: RouteRole::IdeOnly,
    }]
}

/// Documents — read side. NOT nested under `/orgs/{org_id}`, and that is the
/// whole reason these exist at the root: nesting would put `org_middleware` in
/// front, which rejects exactly the frontline workers a Knowledge base is for.
/// The `org_id` therefore arrives as a query parameter and is checked, never
/// trusted, by `visibility::resolve_standing`.
fn read_routes() -> Router<AppState> {
    Router::new()
        .route("/documents", get(handlers::list))
        // Static segment, so the router matches it ahead of `/documents/{id}`
        // rather than reading "search" as a document id.
        .route("/documents/search", get(search::search))
        // The one document route that is NOT on the fleet (see `route_roles`).
        // Kept a separate route from `/documents/search` precisely so that pin
        // does not spread to every search in the product — see `ask`.
        .route("/documents/ask", post(ask::ask))
        // The session store, one segment deeper and on the OTHER side of the
        // fleet split. These read and write Postgres and nothing else, so
        // looking back at what you asked survives the ide restarting even
        // though asking again does not.
        .route(
            "/documents/ask/sessions",
            get(ask_sessions::list).post(ask_sessions::create),
        )
        .route(
            "/documents/ask/sessions/{id}",
            get(ask_sessions::read).delete(ask_sessions::delete),
        )
        .route("/documents/{id}", get(handlers::get))
        .route("/documents/{id}/download", get(handlers::download))
        .route("/documents/{id}/versions", get(versions::list))
        // Reading ONE version — the half the history list could not reach.
        .route("/documents/{id}/versions/{version_no}", get(versions::read))
        .route(
            "/documents/{id}/versions/{version_no}/download",
            get(versions::download),
        )
        .route("/document-folders", get(handlers::list_folders))
        .route("/document-categories", get(categories::list))
        // Favoriting is a personal act on something the caller can already
        // read, so it sits with the reads and takes no org on the path.
        .route(
            "/documents/{id}/favorite",
            post(shelf::favorite).delete(shelf::unfavorite),
        )
}

/// Documents — manage side. Nested under `/orgs/{org_id}` because the
/// `OrgAdmin` extractor needs the org on the path, and because these are the
/// writes. `every_org_scoped_document_write_takes_the_orgadmin_extractor`
/// reads THIS function and every signature it names — being inside it is what
/// puts a handler under that rule.
///
/// Carries `org_middleware` + `subscription_guard`, mirroring oxy-app's
/// `build_org_routes` (where these used to live). `org_middleware` must run
/// BEFORE the guard, which reads the `OrgContext` it inserts; axum applies the
/// last-declared `.layer` outermost, so this order yields
/// `org_middleware → subscription_guard → handler`.
fn org_routes() -> Router<AppState> {
    Router::new()
        .route("/document-folders", post(manage::create_folder))
        .route("/document-categories", post(categories::create))
        .route(
            "/document-categories/{category_id}",
            patch(categories::rename).delete(categories::delete),
        )
        .route(
            "/document-folders/{folder_id}",
            patch(manage::update_folder).delete(manage::trash_folder),
        )
        .route(
            "/document-folders/{folder_id}/restore",
            post(manage::restore_folder),
        )
        .route("/documents", post(manage::create_document))
        .route(
            "/documents/{document_id}",
            patch(manage::update_document).delete(manage::trash_document),
        )
        .route(
            "/documents/{document_id}/restore",
            post(manage::restore_document),
        )
        .route("/documents/{document_id}/review", post(review::decide))
        .route(
            "/documents/{document_id}/pin",
            post(shelf::pin).delete(shelf::unpin),
        )
        .route("/documents/{document_id}/versions", post(versions::create))
        .route(
            "/documents/{document_id}/versions/{version_no}/confirm",
            post(versions::confirm),
        )
        .layer(from_fn(subscription_guard_middleware))
        .layer(from_fn(org_middleware))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// axum 0.8 checks for route conflicts eagerly as a router is built, so
    /// constructing it is enough to catch a duplicate path within this surface.
    #[test]
    fn routes_build_without_conflict() {
        let _ = routes();
    }

    /// Reproduce the merge point against a stand-in of oxy-app's protected
    /// root, which nests its own `/orgs/{org_id}` tree. A collision then
    /// surfaces here, where axum panics eagerly on `.merge`, instead of as a
    /// boot panic. Keep the stand-in in sync with `build_global_routes`.
    #[test]
    fn merges_into_the_protected_root_without_conflict() {
        async fn h() {}
        let _root: Router<AppState> = Router::new()
            .route("/work", get(h))
            .nest("/orgs/{org_id}", Router::new().route("/members", get(h)))
            .merge(routes());
    }
}
