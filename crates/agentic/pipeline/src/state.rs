//! [`AgenticState`]: the agentic runtime state the host shares with every
//! agentic route and with the platform's own state (`oxy-app-core`'s
//! `AppState::agentic_state`).
//!
//! Defined in agentic-runtime (`agentic_runtime::agentic_state`) since
//! 2026-11-27: every field is a runtime type, and the platform state holds
//! it, so it must not pull the facade into the graph
//! (`internal-docs/crate-tiers.md`, stage B). Re-exported here under its old
//! path. What the facade adds is [`new_agentic_state`], which builds it with
//! the domains' event registry.

use std::sync::Arc;

use sea_orm::DatabaseConnection;
use tokio_util::sync::CancellationToken;

pub use agentic_runtime::agentic_state::AgenticState;

use crate::platform::ThreadOwnerLookup;

/// An [`AgenticState`] with the domains' event registry
/// ([`crate::build_event_registry`]): what `AgenticState::new` built before
/// the struct moved down to agentic-runtime.
pub fn new_agentic_state(
    shutdown_token: CancellationToken,
    db: DatabaseConnection,
    thread_owner: Arc<dyn ThreadOwnerLookup>,
) -> AgenticState {
    AgenticState::new(
        shutdown_token,
        db,
        thread_owner,
        crate::build_event_registry(),
    )
}
