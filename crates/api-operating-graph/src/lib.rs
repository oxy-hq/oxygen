//! The operating graph's HTTP surface — locations with their hierarchy and
//! per-integration ids, the positions vocabulary, assignments (who holds which
//! position where, and under whom), and the work items routed along them
//! ([`work`]).
//!
//! [`work`] came first (the assignment graph, #3050); the rest grew beside it
//! rather than into it, because that module is the work-item surface and was
//! already at the file-size guideline. The two share the tables. Design of
//! record: `internal-docs/operating-graph.md`.
//!
//! # Authorization
//!
//! Reads: any org member (`OrgMemberStrict`) — a place is not a secret and
//! every picker needs the list. Writes: `Ring::OrgAdmin` through the `OrgAdmin`
//! extractor, the ring `Action::ManageLocations`, `ManageOrgRoles` and
//! `ManageAssignments` all sit on (the differential tests pin each). The one
//! decision made here and not in the model is the GRANTEE of an assignment: an
//! org member, or an active frontline worker — the same standing the app
//! access settings accept, and the reason a worker can be put on a roster at
//! all.
//!
//! # Where it mounts
//!
//! A sibling crate of `oxy-app`, out of it since 2026-11-14, mounted by the
//! `oxy-server` composition root through `SurfaceSeams::api` ([`routes`] +
//! [`route_roles`]). Two halves, each where it sat in `oxy-app`'s
//! `build_global_routes`: `/work` at the protected root, outside the org tree
//! (a frontline worker holds no org membership, and `org_middleware` would
//! refuse exactly the people work is routed to), and the registry under
//! `/orgs/{org_id}` with `org_middleware` + the subscription guard re-applied.
//! Its guards come from `oxy-server-authz` and its middlewares from
//! `oxy-app-core`, not `oxy-app`; the pure half of the graph (DTOs, readers,
//! the assignment writes, reach, the semantic binding) is
//! `oxy_app_core::operating_graph`, re-exported below.
//!
//! # Fleet role
//!
//! FleetOk throughout, declared by hand in [`route_roles`]: Postgres only. A
//! deploy must not stop a manager rostering a store. Under `route_fleet` the
//! state type refused a working-copy extractor at compile time; across the
//! crate line `undeclared_mounts_stay_diskless` is the check that is left.

// Handlers here await `oxy-app`'s futures, which can pass rustc's default query
// depth in the optimized build (`oxy-server`'s `recursion_limit_guard`).
#![recursion_limit = "256"]

pub mod assignments;
pub mod locations;
pub mod positions;
mod router;
pub mod work;

pub use oxy_app_core::operating_graph::{binding, dto, reach};
pub use router::{route_roles, routes};

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub(crate) fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// Lowered into `oxy-app-core` with the assignment writes that use it.
pub(crate) use oxy_app_core::operating_graph::is_unique_violation;
