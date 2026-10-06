//! An analytics run's execution stamp, for a host that drives one from the
//! task queue.
//!
//! The columns and the statements belong to the analytics domain
//! (`agentic_analytics::extension::execution`, which says what they are for).
//! They are re-exported here because a host enters the agentic subsystem
//! through this crate and may not import a domain crate: `oxy-app`'s queued
//! custom-app ask is the first caller.

pub use agentic_analytics::extension::{
    RunExecution, beat_execution, begin_execution, get_run_execution,
};
