//! GitHub OAuth + git-namespace HTTP surface.
//!
//! Extracted from `oxy-app` as a sibling crate so editing it recompiles only
//! this crate + relinks `oxy-server`, never oxy-app's library. The composition
//! root (`oxy-server`) mounts [`routes`] through the `SurfaceSeams::api` seam,
//! which injects into the protected tree BEFORE `apply_middleware` — so these
//! routes inherit the standard auth stack (auth / api-key / timeout /
//! publish-token-scope). Only the org-scoped inner middleware
//! (`org_middleware` + `subscription_guard`) is re-applied here, matching the
//! original nesting inside oxy-app's `build_org_routes`.
//!
//! The GitHub App's webhook receiver ([`webhook`], `POST /webhooks/github`)
//! rides the other side of the auth stack: [`public_routes`] is merged beside
//! oxy-app's public tree through `SurfaceSeams::public`, because GitHub carries
//! no user credential — the handler authenticates the sender by HMAC instead.
//! It left oxy-app's `build_public_routes` on 2026-11-15, where `route_fleet`
//! declared it `*` FleetOk; [`public_route_roles`] states the same.

// Laying out `get_account`'s future takes 103 of rustc's default query depth of
// 128 at opt-level 3 (measured in #3476), and the `--release` build that produces
// the image takes more — a build PR CI never runs. Raised before it overflows there.
#![recursion_limit = "256"]

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{delete, get, post};

use oxy_app_core::AppState;
use oxy_app_core::guards::org_context::org_middleware;
use oxy_app_core::guards::subscription_guard::subscription_guard_middleware;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

pub mod account;
pub mod attach;
pub mod callback;
pub mod installations;
pub mod namespaces;
pub mod state;
pub mod webhook;

/// Org-scoped routes → `/orgs/{org_id}/github/*`. The leading `{org_id}` segment
/// is captured here (the delete handler reads `Path<(Uuid, Uuid)>` = `(org_id,
/// id)`), so it must stay in the path when mounted.
fn org_github_routes() -> Router<AppState> {
    Router::new()
        .route("/repositories", get(namespaces::list_repositories))
        .route("/branches", get(namespaces::list_branches))
        .route("/namespaces", get(namespaces::list_git_namespaces))
        .route("/namespaces/pat", post(namespaces::create_pat_namespace))
        .route(
            "/namespaces/installation",
            post(namespaces::create_installation_namespace),
        )
        .route("/namespaces/{id}", delete(namespaces::delete_git_namespace))
}

/// Per-user routes → `/user/github/*`. Auth only (inherited from the seam); no
/// org context, so `org_id` travels in the query/body/signed-state instead.
fn user_github_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/account",
            get(account::get_account).delete(account::delete_account),
        )
        .route("/account/oauth-url", get(account::get_oauth_url))
        .route("/installations", get(installations::list_installations))
        .route(
            "/installations/new-url",
            get(installations::get_new_installation_url),
        )
        .route("/callback", post(callback::callback))
}

/// The full GitHub surface, for the composition root to mount via the
/// `SurfaceSeams::api` seam.
///
/// `org_middleware` must run before `subscription_guard` (the guard reads the
/// `OrgContext` the middleware inserts). axum applies the last-declared `.layer`
/// as the outermost, so `.layer(subscription_guard).layer(org_middleware)`
/// yields request order `org_middleware → subscription_guard → handler` — the
/// same order as oxy-app's `build_org_routes`.
pub fn routes() -> Router<AppState> {
    let org = Router::new()
        .nest("/orgs/{org_id}/github", org_github_routes())
        .layer(from_fn(subscription_guard_middleware))
        .layer(from_fn(org_middleware));
    let user = Router::new().nest("/user/github", user_github_routes());
    org.merge(user)
}

/// The unauthenticated half, for the composition root to mount via the
/// `SurfaceSeams::public` seam: the GitHub App's webhook receiver. Relative to
/// `/api`, outside the auth stack — the handler verifies GitHub's signature
/// over the raw body before it reads anything.
pub fn public_routes() -> Router<AppState> {
    Router::new().route("/webhooks/github", post(webhook::github_webhook))
}

/// Pod placement for [`public_routes`], relative to `/api`: method `*`,
/// FleetOk, as `oxy-app`'s `route_fleet` declared it. It reads Postgres and
/// queues a compile task, so any replica serves it.
pub fn public_route_roles() -> &'static [RouteRoleDecl] {
    &[RouteRoleDecl {
        method: "*",
        path: "/webhooks/github",
        role: RouteRole::FleetOk,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The webhook came from `oxy-app`'s public tree, where `route_fleet`'s
    /// type proved the role. Here the role is a declaration, so hold it: the
    /// route is declared exactly once, `*` FleetOk — not merely unclassified,
    /// which also reads FleetOk and is what a dropped row would look like —
    /// and the classifier matches a live URI against it.
    #[test]
    fn the_webhook_is_declared_fleet_ok() {
        use oxy_app_core::pods::role_manifest::declared_role_in;
        let declared = public_route_roles();
        assert_eq!(declared.len(), 1);
        assert_eq!(declared[0].method, "*");
        assert_eq!(declared[0].path, "/webhooks/github");
        assert_eq!(declared[0].role, RouteRole::FleetOk);
        let decls: Vec<_> = declared
            .iter()
            .map(|d| (d.method, format!("/api{}", d.path), d.role))
            .collect();
        assert_eq!(
            declared_role_in(&decls, "POST", "/api/webhooks/github"),
            Some(RouteRole::FleetOk)
        );
    }

    /// The declaration has a route behind it. TRACE, which the webhook does
    /// not take, answers 405 from a mounted path and 404 from an unmounted
    /// one, without running the handler.
    #[tokio::test]
    async fn the_webhook_is_mounted() {
        use axum::body::Body;
        use axum::http::{Method, Request, StatusCode};
        use tower::ServiceExt;

        let router = public_routes().with_state(oxy_app_core::app_state::bare_app_state());
        for (uri, want) in [
            ("/webhooks/github", StatusCode::METHOD_NOT_ALLOWED),
            ("/webhooks/nothing-mounts-this", StatusCode::NOT_FOUND),
        ] {
            let status = router
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
            assert_eq!(status, want, "{uri}");
        }
    }

    /// axum 0.8 checks for route conflicts eagerly as the router is built, so
    /// simply constructing `routes()` (org + user subtrees, both nests, the two
    /// org layers) is enough to catch a duplicate/overlapping path or a bad path
    /// pattern within this surface — no server, no DB, no state.
    #[test]
    fn routes_build_without_conflict() {
        let _ = routes();
    }

    /// The seam merges `routes()` into oxy-app's protected tree, which already
    /// nests `/orgs/{org_id}` and `/user`. That overlap is where a conflict would
    /// actually arise, and the composed router is otherwise only built from
    /// `main.rs` (never in a test). Reproduce the merge here so a conflict surfaces
    /// in CI — axum panics eagerly on `.merge` — instead of as a boot panic in prod.
    ///
    /// The protected tree below is a hand-maintained STAND-IN for oxy-app's real
    /// `build_org_routes` / `build_global_routes` (which take a DB-backed AppState,
    /// so the real tree can't be built cheaply in a unit test). Keep it in sync: if
    /// oxy-app adds a `/user/github/…` or `/orgs/{org_id}/github/…` path, add it here
    /// too, or this test passes while the conflict resurfaces at boot — the real
    /// end-to-end guard (`smoke-test-enterprise`) is still `if: false` in CI.
    #[test]
    fn merges_into_protected_tree_without_conflict() {
        use axum::routing::{delete, get};
        async fn h() {}
        let protected: Router<AppState> = Router::new()
            .nest(
                "/orgs/{org_id}",
                Router::new()
                    .route("/members", get(h))
                    .route("/workspaces/{id}", delete(h)),
            )
            .nest("/user", Router::new().route("/settings", get(h)));
        let _merged: Router<AppState> = protected.merge(routes());
    }
}
