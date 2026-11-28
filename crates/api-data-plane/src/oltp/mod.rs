//! Axum routes for the per-org OLTP database. Was `oxy_oltp::api` until
//! crate-depth B2 took the HTTP surface out of `oxy-oltp`, so that domain crate
//! no longer depends on `oxy-auth`.
//!
//! - `GET /oltp/me/connection` — status, schemas, and analyst readiness. Safe;
//!   no side effects; **no credentials**.
//!
//! There is deliberately no credential endpoint. Airhouse has one because a
//! warehouse is something users connect their own tools to; a per-org OLTP
//! database is not. Queries go through the IDE via `type: postgres_managed`,
//! which resolves the read-only analyst server-side.

pub mod admin;
pub mod apply;
pub mod branches;
pub mod confinement;
pub mod erd;
pub mod handlers;
pub mod probe;

use axum::Router;
use axum::http::StatusCode;
use axum::routing::get;

/// A `ResolveError` as an HTTP status, shared by every route that resolves an
/// OLTP connection. Absent is 404, mid-transition 409, broken 500 — so one
/// error cannot read as 409 on one route and 404 on another.
pub(crate) fn resolve_status(e: oxy_oltp::resolver::ResolveError) -> (StatusCode, String) {
    use oxy_oltp::resolver::ResolveError as R;
    let code = match &e {
        R::WorkspaceNotFound(_)
        | R::WorkspaceHasNoOrg(_)
        | R::NotProvisioned(_)
        | R::NoAnalystCredential(_)
        | R::WriterNotProvisioned { .. }
        | R::BranchCredentialMissing { .. }
        | R::BranchOwnerCredentialMissing(..)
        | R::EnvBranchCredentialMissing { .. } => StatusCode::NOT_FOUND,
        R::NotActive(..)
        | R::BranchNotActive(..)
        | R::BranchIsProduction(..)
        | R::EnvBranchNotReady { .. }
        | R::EnvBranchIsProduction { .. } => StatusCode::CONFLICT,
        R::Db(_) | R::Crypto(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, e.to_string())
}

/// Build the `/oltp/me/*` route subtree.
///
/// Generic over the host's state type — handlers reach the database through
/// `oxy_platform::db::establish_connection`, exactly as airhouse's do. Mount
/// with `app_router.merge(oxy_api_data_plane::oltp::router())`.
pub fn router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        // Spelled from the crate root: `crate::airhouse::handlers` has a
        // `get_connection` too, and the route catalog matches a handler's doc
        // by module suffix, so a bare `handlers::` would be ambiguous there.
        .route(
            "/oltp/me/connection",
            get(crate::oltp::handlers::get_connection),
        )
        // Structure only — no row data, and read as the analyst. See erd.rs on
        // why this is not the generic /databases/{name}/schema endpoint.
        .route("/oltp/me/erd", get(erd::get_erd))
}
