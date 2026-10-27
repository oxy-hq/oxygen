//! Run lifecycle, persistence, and event streaming.
//!
//! Owns everything that survives between fan-out boundaries: the
//! `agentic_runs` row, its event log, suspensions, the in-memory
//! [`state::RuntimeState`] that wakes streaming subscribers, and the
//! domain-agnostic [`handle::PipelineHandle`] / bridge plumbing.
//!
//! Stage 1 of the airway/airform extraction split runtime into this
//! "what a run *is*" layer and the [`crate::orchestrator`] "how a run
//! *executes*" layer. Lifecycle has zero orchestrator dependencies —
//! orchestrator depends on lifecycle for storage primitives.

pub mod bridge;
pub mod crud;
pub mod done_hook;
pub mod driver_lease;
pub mod entity;
pub mod event_registry;
pub mod failure_hook;
pub mod handle;
pub mod notice_request;
pub mod state;
pub mod suspend_hook;
