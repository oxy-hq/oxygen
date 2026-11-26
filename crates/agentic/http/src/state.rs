//! The agentic routes' shared state. [`AgenticState`] is defined in
//! `agentic-pipeline` (`agentic_pipeline::state`) so the platform state can
//! hold it without depending on this transport crate; it is re-exported here
//! under its old path.

pub use agentic_pipeline::state::AgenticState;
pub use agentic_runtime::state::{RunError, RunStatus, RuntimeState};
