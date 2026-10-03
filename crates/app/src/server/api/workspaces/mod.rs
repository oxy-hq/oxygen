//! Workspace management HTTP surface: git operations (pull/push/fetch, branch
//! switching, conflict resolution, rebase/reset), workspace details/status,
//! and workspace CRUD (list/rename/delete).
//!
//! - [`dto`]: request/response serde types shared across handlers.
//! - [`ops`]: internal git + filesystem helpers used by the handlers.
//! - [`detached`]: what the surface does on a detached HEAD (no branch).
//! - [`handlers`]: the HTTP handler functions themselves.

pub(crate) mod detached;
mod dto;
mod handlers;
mod ops;

pub use detached::GitRefusal;
pub use dto::*;
pub use handlers::*;
pub use ops::cleanup_workspace_schedules;
pub use ops::{
    build_workspace_details_response_for_uninitialized_local, compute_workspace_storage_key,
};
