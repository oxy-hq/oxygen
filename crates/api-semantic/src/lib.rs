//! The semantic model's HTTP surface: the IDE's semantic explorer (a view's
//! and a topic's details, compile and execute), the Dev Portal's SQL
//! execution, the pre-aggregation status and rebuild, and the compile
//! boundary's revision reads (the served revision, its files, the semantic
//! object list and the Context Graph's sources). Out of `oxy-app`'s
//! `server/api/{semantic, semantic_scan, data, preagg}.rs` on 2026-11-16 (Wave
//! D2 in `internal-docs/domain-boundaries.md`).
//!
//! Two trees mount it, as they mounted these handlers when they lived in
//! `oxy-app`'s `build_workspace_routes` and `build_external_workspace_routes`:
//!
//! - [`workspace_routes`] rides the workspace seam, merged INSIDE the
//!   `/api/{workspace_id}` nest, so it inherits the session auth stack and
//!   `workspace_middleware`.
//! - [`external_routes`] rides the external seam, merged into the curated
//!   `/external/api/{workspace_id}` tree before its API-key-only auth, wide-open
//!   CORS and workspace context layers: `POST /sql/query`, `POST /semantic` and
//!   `POST /semantic/compile`, the three a standalone app calls.
//!
//! Every route is FleetOk, as `route_fleet` declared each of them in `oxy-app`
//! (method `*`). Every handler takes `WorkspaceManagerReadOnly`, so none can
//! reach for the working copy: the scan resolves through the compile boundary
//! first ([`oxy_app::surface::workspace_runtime::resolve_query_scan_source`]),
//! the revision reads are compiled rows, and the pre-aggregation cache is
//! keyed on the workspace id with a cross-node read-through from the blob
//! bucket. `WorkspaceManagerWorkingCopy` resolves only from `IdeState`, so a
//! handler here that asked for it would not compile against `AppState`: the
//! type gate `route_fleet` gave survives the move.
//!
//! The SQL execution itself (the connector, the 10k-row cap and `truncated`,
//! the preview write hold) and the error shaping of an execute are `oxy-app`'s
//! workspace runtime (`agentic_wiring::sql_exec`), reached through
//! [`oxy_app::surface::world_model_exec`]; the scan resolution is
//! `oxy_workspace_runtime::scan_source`, reached through
//! [`oxy_app::surface::workspace_runtime`]. The pre-aggregation context a
//! semantic execute reads rollups through is `oxy_app_core::preagg_context`
//! (display posture: `ServeStale`), and the ledger of rollups this node
//! rebuilt to zero rows is `oxy_compile::preagg_ledger`.
//!
//! [`openapi`] carries `POST /{workspace_id}/sql/query`, the operation the
//! curated OpenAPI document has always listed, through `SurfaceSeams::openapi`.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

pub mod data;
pub mod preagg;
pub mod semantic;
pub mod semantic_scan;

use axum::Router;
use axum::routing::{get, post};
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The workspace seam's share: merged inside `/api/{workspace_id}`, where
/// `oxy-app`'s `build_workspace_routes` mounted each of these with
/// `route_fleet`.
///
/// - `/sql/{pathb64}`, `/sql/query`: the Dev Portal's SQL execute.
/// - `/semantic/objects`: the views and topics the workspace serves, for the
///   explorer's list and the pickers — the revision (or single-process mode's
///   directory).
/// - `/context-graph/sources`: the Context Graph page's views, topics,
///   automations, agents and verified-query paths, from the served revision
///   (it read `/files`).
/// - `/revision`, `/revision/files`, `/revision/files/content`: the revision
///   this request reads and its commit, for the top bar, and the Factory's
///   read-only file browser over the compiled rows (it read `/files`).
/// - `/semantic/topic/{file_path_b64}`, `/semantic/view/{file_path_b64}`,
///   `/semantic/compile`, `/semantic`: the semantic explorer. The legacy
///   `/semantic` execute route was retired alongside `oxy-workflow`; execution
///   flows through the agentic pipeline, and compile reaches airlayer through
///   `agentic_semantic`.
/// - `/semantic/preagg-status`, `/semantic/preagg-rebuild`: moved to
///   `api::preagg` by #2989, which also added the rebuild verb. Both FleetOk:
///   the list comes from the DECLARATIONS (resolved through the compile
///   boundary first), and a rollup built on another node is readable here when
///   a blob bucket is configured.
///
/// No comment sits between the mounts: the route catalog reads one as the
/// route's note, and `oxy-app`'s `route_fleet` mounts carried none, so the
/// generated table stays as it was.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .route("/sql/{pathb64}", post(data::execute_sql))
        .route("/sql/query", post(data::execute_sql_query))
        .route(
            "/semantic/objects",
            get(semantic_scan::list_semantic_objects),
        )
        .route(
            "/context-graph/sources",
            get(semantic_scan::get_context_graph_sources),
        )
        .route("/revision", get(semantic_scan::get_served_revision))
        .route("/revision/files", get(semantic_scan::get_revision_files))
        .route(
            "/revision/files/content",
            get(semantic_scan::get_revision_file),
        )
        .route(
            "/semantic/topic/{file_path_b64}",
            get(semantic::get_topic_details),
        )
        .route(
            "/semantic/view/{file_path_b64}",
            get(semantic::get_view_details),
        )
        .route("/semantic/preagg-status", get(preagg::get_preagg_status))
        .route("/semantic/preagg-rebuild", post(preagg::rebuild_preagg))
        .route("/semantic/compile", post(semantic::compile_semantic_query))
        .route("/semantic", post(semantic::execute_semantic_query))
}

/// The external seam's share: merged inside `/external/api/{workspace_id}`,
/// where `oxy-app`'s `build_external_workspace_routes` mounted the same three
/// handlers, so behaviour is identical and only the auth and CORS wrapper
/// differs.
pub fn external_routes() -> Router<AppState> {
    Router::new()
        .route("/sql/query", post(data::execute_sql_query))
        .route("/semantic", post(semantic::execute_semantic_query))
        .route("/semantic/compile", post(semantic::compile_semantic_query))
}

/// `route_fleet`'s declaration, which is what each of these was in `oxy-app`:
/// method `*`, FleetOk.
const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree.
/// The external tree carries no declarations (its curated router drops them);
/// every route there falls to the FleetOk default, which is the truth here.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/sql/{pathb64}"),
    fleet("/sql/query"),
    fleet("/semantic/objects"),
    fleet("/context-graph/sources"),
    fleet("/revision"),
    fleet("/revision/files"),
    fleet("/revision/files/content"),
    fleet("/semantic/topic/{file_path_b64}"),
    fleet("/semantic/view/{file_path_b64}"),
    fleet("/semantic/preagg-status"),
    fleet("/semantic/preagg-rebuild"),
    fleet("/semantic/compile"),
    fleet("/semantic"),
];

/// The operation this surface documents in the served OpenAPI document
/// (`/apidoc/openapi.json`, which `oxyc schema` reads), merged in by the
/// composition root through `SurfaceSeams::openapi`: `POST
/// /{workspace_id}/sql/query`, the one `oxy-app`'s `openapi_router` listed.
pub fn openapi() -> utoipa::openapi::OpenApi {
    utoipa_axum::router::OpenApiRouter::<AppState>::new()
        .routes(utoipa_axum::routes!(data::execute_sql_query))
        .into_openapi()
}

#[cfg(test)]
mod tests;
