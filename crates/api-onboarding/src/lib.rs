//! Onboarding HTTP surface: workspace creation (demo / new / GitHub import),
//! LLM-key and warehouse-credential readiness checks, onboarding reset
//! ("start over"), and warehouse data-file uploads.
//!
//! - [`dto`]: request/response serde types shared across handlers.
//! - [`ops`]: internal database, filesystem, and multipart helpers plus the
//!   upload size constants.
//! - [`handlers`]: the HTTP handler functions themselves.

mod dto;
mod handlers;
mod ops;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::middleware::from_fn;
use axum::routing::{get, post};
use oxy_app::surface::org_middleware;
use oxy_app::surface::subscription_guard_middleware;
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

pub use handlers::*;
pub use ops::MAX_UPLOAD_BODY_BYTES;

/// Org-scoped onboarding routes: create a workspace from a demo seed, a blank
/// project, or a GitHub import. These live at `/orgs/{org_id}/onboarding/*` and
/// carry `org_middleware` + `subscription_guard`, mirroring oxy-app's
/// `build_org_routes` (where they used to live): `org_middleware` reads the
/// `{org_id}` path param and inserts the `OrgContext` that `OrgAdmin` — and
/// `setup_github`'s cross-org namespace check — depend on, and the guard keeps
/// workspace creation paywall-gated.
///
/// `org_middleware` must run BEFORE `subscription_guard` (the guard reads the
/// `OrgContext` the middleware inserts). axum applies the last-declared `.layer`
/// as outermost, so `.layer(subscription_guard).layer(org_middleware)` yields
/// request order `org_middleware → subscription_guard → handler` — the same order
/// as `build_org_routes`. Merged at the protected-tree root by `oxy-server` via
/// the `SurfaceSeams::api` seam.
///
/// These clone repos and scaffold `config.yml` onto node-local disk, so
/// `role_manifest.rs` pins `POST /api/orgs/{org_id}/onboarding/{demo,new,github}`
/// as **IdeOnly** — the path shape here must stay exactly that for the
/// path-keyed classifier to match (a bare `/onboarding/*` silently falls through
/// to FleetOk and lets a stateless serve replica clone into a checkout it doesn't
/// own).
/// Every route this crate mounts, and which pod may serve it.
///
/// Stated here rather than at the mount because `oxy-app` owns `RoleRouter` and
/// cannot see these handlers — the same reason `agentic-http` carries
/// `router_roles()`. Paths are absolute: both seams MERGE this crate rather than
/// nesting it, so there is no prefix to join against.
///
/// All IdeOnly, and none of it is a judgement call. `demo`/`new`/`github` clone
/// a repository and scaffold `config.yml` through raw `std::fs` and git; reset
/// rewrites that checkout; the readiness and key checks read it; and uploads
/// write warehouse files into it. Not one of them can be answered by a replica
/// with no working copy, and the failure would be silent — a clone into a
/// directory the process does not own.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    const IDE: RouteRole = RouteRole::IdeOnly;
    &[
        RouteRoleDecl {
            method: "POST",
            path: "/orgs/{org_id}/onboarding/demo",
            role: IDE,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/orgs/{org_id}/onboarding/new",
            role: IDE,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/orgs/{org_id}/onboarding/github",
            role: IDE,
        },
    ]
}

/// The workspace-scoped half, merged inside the `/{workspace_id}` nest. Paths
/// are relative to that nest, which is where the seam merges them.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    const IDE: RouteRole = RouteRole::IdeOnly;
    &[
        RouteRoleDecl {
            method: "GET",
            path: "/onboarding-readiness",
            role: IDE,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/onboarding/github-setup",
            role: IDE,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/onboarding/reset",
            role: IDE,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/onboarding/test-llm-key",
            role: IDE,
        },
        RouteRoleDecl {
            method: "POST",
            path: "/onboarding/upload-warehouse-files",
            role: IDE,
        },
    ]
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .nest("/orgs/{org_id}/onboarding", global_onboarding_routes())
        .layer(from_fn(subscription_guard_middleware))
        .layer(from_fn(org_middleware))
}

fn global_onboarding_routes() -> Router<AppState> {
    Router::new()
        .route("/demo", post(setup_demo))
        .route("/new", post(setup_new))
        .route("/github", post(setup_github))
}

/// Workspace-scoped onboarding routes (readiness checks, reset, LLM-key test,
/// warehouse file uploads). Merged INSIDE the `/{workspace_id}` nest by
/// `oxy-server` via the workspace seam, so they inherit `workspace_middleware`
/// (which resolves the workspace context) exactly as they did when they lived in
/// `build_workspace_routes`.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .route("/onboarding-readiness", get(onboarding_readiness))
        .route("/onboarding/github-setup", get(github_setup))
        .nest("/onboarding", build_onboarding_routes())
}

/// The upload-bearing subtree. `DefaultBodyLimit` is applied on the whole nested
/// Router (not an individual `MethodRouter`) — the latter can interact
/// unexpectedly with outer CORS preflight handling on axum 0.8.
fn build_onboarding_routes() -> Router<AppState> {
    Router::new()
        .route("/reset", post(reset_onboarding))
        .route("/test-llm-key", post(test_llm_key))
        .route("/upload-warehouse-files", post(upload_warehouse_files))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_BODY_BYTES))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// axum 0.8 checks for route conflicts eagerly as a router is built, so just
    /// constructing each seam's Router is enough to catch a duplicate/overlapping
    /// path or a bad pattern within this surface — no server, no DB, no state.
    #[test]
    fn routes_build_without_conflict() {
        let _ = routes();
        let _ = workspace_routes();
    }

    /// This surface rides BOTH seams, so reproduce both merge points against
    /// stand-ins of oxy-app's real trees — a conflict (or a mis-shaped path that
    /// collides with the org / workspace tree) then surfaces here in CI, where
    /// axum panics eagerly on `.merge`, instead of as a boot panic in prod. The
    /// composed router is otherwise only built from `main.rs`, never in a test.
    ///
    /// The stand-ins are hand-maintained substitutes for `build_global_routes` /
    /// `build_workspace_routes` (which take a DB-backed AppState and can't be built
    /// cheaply here); keep them in sync. `routes()` MUST nest under `/orgs/{org_id}`
    /// — `org_middleware` needs the `{org_id}` param and `role_manifest` pins these
    /// `IdeOnly` by that exact path — so the global stand-in mounts the org tree it
    /// merges into.
    #[test]
    fn merges_into_the_two_seam_trees_without_conflict() {
        use axum::routing::get;
        async fn h() {}

        // SurfaceSeams::api seam: the protected-tree root, which nests /orgs/{org_id}.
        let root: Router<AppState> = Router::new()
            .nest("/orgs/{org_id}", Router::new().route("/members", get(h)))
            .merge(routes());

        // SurfaceSeams::workspace seam: inside the /{workspace_id} nest.
        let workspace: Router<AppState> = Router::new()
            .route("/details", get(h))
            .merge(workspace_routes());

        let _ = (root, workspace);
    }
}
