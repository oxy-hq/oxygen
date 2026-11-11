//! Airway's HTTP surface (Pending 5 in `internal-docs/domain-boundaries.md`).
//!
//! Three routes, all merged INSIDE the `/{workspace_id}` nest by `oxy-server`
//! through the workspace seam, so they inherit `workspace_middleware` exactly
//! as they did when they lived in oxy-app's `build_workspace_routes`:
//!
//! - `POST /source-uploads/reports` — report uploads for file-based sources,
//!   into the shared landing zone ([`source_upload`]);
//! - `GET /airway-pipelines` — the workspace's pipelines, from the compile
//!   boundary ([`pipelines`]);
//! - `GET /airway-pipelines/sp-api/marketplaces` — the `sp_api` connector's
//!   marketplaces, for the New Pipeline wizard ([`pipelines`]).
//!
//! The run surface (`/agentic-airway`) stays in `agentic-http`. The staff
//! console's `/admin/airway` section and the `oxy airway` command join next.

// Laying out `upload_report`'s future exceeds rustc's default query depth of
// 128 in the `--release` build that produces the image — and only there: a
// dev-profile build stays under it, so PR CI passes. rustc's own suggestion.
#![recursion_limit = "256"]

mod pipelines;
mod source_upload;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

use source_upload::{MAX_REPORT_BYTES, upload_report};

/// Public only because the module docs link to it: nothing outside this crate
/// reads it, and a private const referenced solely from doc links is dead code.
pub use source_upload::ZONE_VAR;

/// Every route this crate mounts, and which pod may serve it. Paths are
/// relative to the `/{workspace_id}` nest the seam merges into.
///
/// FleetOk, declared rather than defaulted: the upload writes to S3 and reads
/// nothing node-local, so pinning it to the singleton would cost HA for no
/// reason. It is deliberately NOT under `/agentic-airway`, which is an
/// `IdeOnly` `{*rest}` wildcard so a live run stays on the instance holding
/// the working copy — the carve-out is the upload, not the surface.
///
/// The two pipeline reads are FleetOk for the reason they always were: the
/// listing reads the compile boundary (`WorkspaceManagerReadOnly`, so the
/// compiler refuses a working-copy read), and the marketplace list is static.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &[
        RouteRoleDecl {
            method: "POST",
            path: "/source-uploads/reports",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/airway-pipelines",
            role: RouteRole::FleetOk,
        },
        RouteRoleDecl {
            method: "GET",
            path: "/airway-pipelines/sp-api/marketplaces",
            role: RouteRole::FleetOk,
        },
    ]
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/airway-pipelines", get(pipelines::list_pipelines))
        // Static connector metadata for the New Pipeline wizard's marketplace
        // picker. Served rather than duplicated in the frontend so the list
        // has one source of truth (`source_factory::NA_MARKETPLACES`).
        .route(
            "/airway-pipelines/sp-api/marketplaces",
            get(pipelines::list_sp_api_marketplaces),
        )
        .merge(upload_routes())
}

/// The upload, with the body limit it needs and the reads do not.
fn upload_routes() -> Router<AppState> {
    Router::new()
        .route("/source-uploads/reports", post(upload_report))
        // Without this, axum 0.8's 2 MiB default governs and the handler's own
        // ceiling is unreachable — a larger report fails inside `field.bytes()`
        // as a 400, never the 413 the handler writes. At `Router` level rather
        // than on the `MethodRouter` for the same reason `oxy-api-tenancy`'s `onboarding` module
        // gives: the latter can interact unexpectedly with outer CORS preflight
        // handling on axum 0.8.
        // `MAX_REPORT_BYTES` plus slack, because this bounds the whole
        // multipart body while the constant bounds ONE FILE: boundaries, field
        // names, `pipeline_ref`, `workflow_id` and the period all ride along.
        // Set equal, a file a few hundred bytes under the ceiling passed the
        // client-side check and the handler's own check and still died here —
        // answered by this layer's terse 400, never the handler's 413 that
        // names the size. The handler stays the authority on the file itself.
        .layer(DefaultBodyLimit::max(MAX_REPORT_BYTES + 64 * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// axum 0.8 checks for route conflicts eagerly, so reproduce the seam's
    /// merge point against a stand-in of the `/{workspace_id}` tree — a
    /// collision then fails here instead of as a boot panic in prod.
    #[test]
    fn merges_into_the_workspace_tree_without_conflict() {
        use axum::routing::get;
        async fn h() {}

        let _: Router<AppState> = Router::new()
            .route("/details", get(h))
            .route("/agentic-airway/{*rest}", get(h))
            .merge(routes());
    }
}
