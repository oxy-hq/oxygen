//! The agentic routes' shared state. [`AgenticState`] is defined in
//! `agentic-runtime` (`agentic_runtime::agentic_state`) so the platform state
//! can hold it without depending on this transport crate or the facade; it is
//! re-exported here under its old path, with the facade's constructor
//! [`new_agentic_state`].

pub use agentic_pipeline::state::{AgenticState, new_agentic_state};
pub use agentic_runtime::state::{RunError, RunStatus, RuntimeState};
