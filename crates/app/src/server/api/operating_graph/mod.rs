//! The operating graph's HTTP surface — locations with their hierarchy and
//! per-integration ids, the positions vocabulary, and assignments (who holds
//! which position where, and under whom).
//!
//! Grown beside `work/` (the assignment graph, #3050) rather than into it: that
//! module is the work-item surface and was already at the file-size guideline.
//! The two share the tables. Design of record: `internal-docs/operating-graph.md`.
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
//! # Fleet role
//!
//! `route_fleet` throughout: Postgres only. A deploy must not stop a manager
//! rostering a store.

pub mod assignments;
pub mod binding;
pub mod dto;
pub mod locations;
pub mod positions;
pub mod reach;

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

pub(crate) fn json_error(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(serde_json::json!({ "error": message.into() }))).into_response()
}

/// Lowered into `oxy-app-core` with the assignment writes that use it.
pub(crate) use oxy_app_core::operating_graph::is_unique_violation;
