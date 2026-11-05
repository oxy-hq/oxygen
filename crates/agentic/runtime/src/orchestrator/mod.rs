//! FSM execution infrastructure: task queue, coordinator, worker pool,
//! transports, and the postgres LISTEN/NOTIFY router.
//!
//! Owns the "how a run executes" half of the runtime — the agent that
//! drains the durable task queue, fans children out, accumulates their
//! results, and resumes parents. Built on top of [`crate::lifecycle`]
//! for persistence; this layer never touches lifecycle internals
//! directly except through that crate's CRUD API.

pub mod background;
pub mod coordinator;
pub mod crud;
pub mod entity;
pub mod router;
pub mod transport;
pub mod worker;
