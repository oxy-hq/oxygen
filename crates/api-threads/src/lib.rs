//! The conversation surface, out of `oxy-app` (Wave B of the core routes,
//! `internal-docs/domain-boundaries.md`): what a person reads and drives a
//! conversation through, the Conversation aggregate's HTTP face.
//!
//! - [`thread`]: a workspace's threads (list, create, read, stop, delete one,
//!   all or a batch) and the run logs (`/logs`).
//! - [`agent`]: the agent listing the chat panel's selector reads.
//! - [`chat`]: org chat — channels, messages and the live stream — and the
//!   cross-replica delivery listener that wakes this replica's streams for a
//!   message another replica took.
//! - [`artifacts`], [`chart`], [`result_files`]: what a run left behind for
//!   the conversation to render — an artifact row, a legacy chart file and a
//!   result parquet file. The two files are written to one pod's disk and
//!   mirrored to S3, so any replica reads them (`surface::runtime_artifact`).
//!
//! [`workspace_routes`] rides the workspace seam, merged INSIDE the
//! `/api/{workspace_id}` nest, so it inherits the session auth stack, the
//! workspace subscription guard and `workspace_middleware`, where
//! `build_workspace_routes` mounted these. Every route is FleetOk, method `*`,
//! as `route_fleet` declared them: thread and run-history reads are persisted
//! data and must serve from any replica. No handler here takes a working
//! copy (`WorkspaceManagerReadOnly` only).
//!
//! [`routes`] (chat) rides the api seam, merged at the protected root where
//! `build_global_routes` had it: cloud only, behind the standard auth stack,
//! no org gate in the path (membership is in every query; see [`chat`]).
//! Every chat route is FleetOk, the SSE stream included. [`background`]
//! starts the delivery listener with the surfaces' other jobs.
//!
//! [`openapi`] carries the thread and agent operations the curated OpenAPI
//! document has always listed, through `SurfaceSeams::openapi`.
//!
//! [`admin`] is the staff console's Explorer (cross-tenant thread and run
//! search), riding `SurfaceSeams::admin` behind `PlatformExplorer` — moved out of
//! `oxy-app`'s `admin::explorer` in the admin split.

// The same headroom every surface crate takes (see
// `crates/server/src/recursion_limit_guard.rs`): an async handler's future can
// exceed rustc's default query depth in the `--release` image build only.
#![recursion_limit = "256"]

pub mod admin;
pub mod agent;
pub mod artifacts;
pub mod chart;
pub mod chat;
pub mod result_files;
pub mod thread;

use axum::Router;
use axum::routing::{get, post};
use oxy_app::surface::seams::BackgroundSeam;
use oxy_app_core::AppState;
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The staff-console section this crate contributes under `/api/admin`, for
/// `oxy-server` to merge, and its pod placement.
pub use admin::{admin_sections, route_roles as admin_route_roles};

/// The admin section's routes, relative to `/admin`, here rather than in
/// [`admin`]: the route catalog's seed (`admin_routes` in this module) resolves
/// functions only, and a delegating `admin::routes()` would resolve to another
/// crate's same-named builder (the walker breaks a tie by directory, and this
/// file shares none with either).
pub fn admin_routes() -> Router<AppState> {
    admin::explorer::router()
}

/// The api seam's share: org chat, merged at the protected root (`/api`).
///
/// Every route is FleetOk, INCLUDING the SSE stream, and that is the
/// interesting call. The route-classification skill pins live streams to the
/// ide — but that rule is about runs executing in-process against a working
/// copy. This stream is a fan-out over persisted data, woken by Postgres
/// LISTEN/NOTIFY, and touches no working copy, no `.git` and no state dir.
///
/// Pinning it to the singleton would be actively wrong twice over: chat would
/// die on every deploy, and it would only work at all for whoever happened to
/// be routed to the ide. This is the "reads must stay HA" half of that skill,
/// and a conversation is the most read-shaped thing in the product.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/chat/channels",
            get(chat::handlers::list_channels).post(chat::handlers::create_channel),
        )
        .route(
            "/chat/channels/{id}/join",
            post(chat::handlers::join_channel),
        )
        .route(
            "/chat/channels/{id}/messages",
            get(chat::handlers::list_messages).post(chat::handlers::post_message),
        )
        .route("/chat/channels/{id}/read", post(chat::handlers::mark_read))
        .route("/chat/channels/{id}/stream", get(chat::handlers::stream))
}

/// Pod placement for [`routes`], relative to `/api`: the five rows
/// `oxy-app`'s `route_fleet` declared, method `*`, FleetOk.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    ROUTE_ROLES
}

const ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/chat/channels"),
    fleet("/chat/channels/{id}/join"),
    fleet("/chat/channels/{id}/messages"),
    fleet("/chat/channels/{id}/read"),
    fleet("/chat/channels/{id}/stream"),
];

/// The job a serve process starts for chat: its delivery listener
/// (`chat_delivery_listener`, which `oxy-app` refuses to boot without).
///
/// It rides the task router's connection settings — the same factory and TLS
/// posture, built from the same environment (`surface::listener`). Two
/// people in one channel are routinely on different replicas, so an
/// in-process bus would deliver a message to whoever happened to hit the
/// replica that accepted the POST and to nobody else — invisible on a dev
/// box, total in production. Started once per process
/// ([`chat::delivery::start_listener`] guards it), whatever the caller does,
/// and bound to the process's shutdown signal. Returns no handle: nothing
/// needs flushing on shutdown.
pub fn background() -> Vec<BackgroundSeam> {
    vec![BackgroundSeam {
        name: "chat_delivery_listener",
        start: |deps| {
            use oxy_app::surface::listener::{
                listener_factory_from_env, listener_tls_verification_from_env,
            };
            match (
                listener_factory_from_env(),
                listener_tls_verification_from_env(),
            ) {
                (Ok(factory), Ok(tls)) => {
                    chat::delivery::start_listener(factory, tls, deps.shutdown.clone())
                }
                (Err(e), _) | (_, Err(e)) => {
                    tracing::warn!(error = %e, "chat delivery listener not started")
                }
            }
            None
        },
    }]
}

/// The workspace seam's share: merged inside `/api/{workspace_id}`.
///
/// `/threads` and `/agents` are nested, as `oxy-app` nested them, so each
/// prefix answers exactly what it did (the list is its `/`). No comment sits
/// directly above a `.route(`: the route catalog would read one as the route's
/// note, and these routes had none.
pub fn workspace_routes() -> Router<AppState> {
    Router::new()
        .nest("/threads", thread_routes())
        .nest("/agents", agent_routes())
        .route("/artifacts/{id}", get(artifacts::get_artifact))
        .route("/charts/{file_path}", get(chart::get_chart))
        .route("/logs", get(thread::get_logs))
        .route(
            "/results/files/{file_id}",
            get(result_files::get_result_file).delete(result_files::delete_result_file),
        )
}

fn thread_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/",
            get(thread::get_threads)
                .post(thread::create_thread)
                .delete(thread::delete_all_threads),
        )
        .route("/bulk-delete", post(thread::bulk_delete_threads))
        .route(
            "/{id}",
            get(thread::get_thread).delete(thread::delete_thread),
        )
        .route("/{id}/stop", post(thread::stop_thread))
}

fn agent_routes() -> Router<AppState> {
    Router::new().route("/", get(agent::get_agents))
}

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: RouteRole::FleetOk,
    }
}

/// Pod placement for [`workspace_routes`], relative to the workspace tree:
/// nine rows, method `*`, FleetOk. `oxy-app`'s `route_fleet` declared ten;
/// the tenth, `/builder-availability`, was deleted with the builder agent. A
/// nested `/` declares the prefix itself (`/threads`, `/agents`), as
/// `RoleRouter::nest` joined it.
pub fn workspace_route_roles() -> &'static [RouteRoleDecl] {
    WORKSPACE_ROUTE_ROLES
}

/// A `const`, not a literal in the function: `fleet` is a `const fn`, and a
/// slice of its calls is promoted to `'static` only through a constant.
const WORKSPACE_ROUTE_ROLES: &[RouteRoleDecl] = &[
    fleet("/threads"),
    fleet("/threads/bulk-delete"),
    fleet("/threads/{id}"),
    fleet("/threads/{id}/stop"),
    fleet("/agents"),
    fleet("/artifacts/{id}"),
    fleet("/charts/{file_path}"),
    fleet("/logs"),
    fleet("/results/files/{file_id}"),
];

/// The operations this surface documents in the served OpenAPI document
/// (`/apidoc/openapi.json`, which `oxyc schema` reads), merged in by the
/// composition root through `SurfaceSeams::openapi`. The same nine
/// `oxy-app`'s `openapi_router` listed: the agent listing, the thread routes
/// and the run logs.
pub fn openapi() -> utoipa::openapi::OpenApi {
    utoipa_axum::router::OpenApiRouter::<AppState>::new()
        .routes(utoipa_axum::routes!(agent::get_agents))
        .routes(utoipa_axum::routes!(thread::get_threads))
        .routes(utoipa_axum::routes!(thread::get_thread))
        .routes(utoipa_axum::routes!(thread::create_thread))
        .routes(utoipa_axum::routes!(thread::delete_thread))
        .routes(utoipa_axum::routes!(thread::delete_all_threads))
        .routes(utoipa_axum::routes!(thread::stop_thread))
        .routes(utoipa_axum::routes!(thread::bulk_delete_threads))
        .routes(utoipa_axum::routes!(thread::get_logs))
        .into_openapi()
}

#[cfg(test)]
mod tests;
