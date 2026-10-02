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

use axum::Router;
use axum::middleware::from_fn;
use axum::routing::{delete, get, post};

use oxy_app::surface::org_middleware;
use oxy_app::surface::subscription_guard_middleware;
use oxy_app_core::AppState;

pub mod account;
pub mod callback;
pub mod installations;
pub mod namespaces;
pub mod state;

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

#[cfg(test)]
mod tests {
    use super::*;

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
