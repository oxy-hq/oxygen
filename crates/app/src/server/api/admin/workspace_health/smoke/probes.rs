//! The three smoke probes. Each takes a workspace context and one target, does
//! the smallest real thing that proves the target works, and returns `Ok(())` or
//! a [`ProbeFailure`]. Timing, timeouts, and verdict construction all live in
//! `runner.rs` — a probe never builds a verdict itself.

mod agent;
mod connection;
mod semantic;

pub(super) use agent::ask;
pub(super) use connection::ping;
pub(super) use semantic::{SemanticTarget, plan, plan_selected, query};

/// Why a probe didn't pass. The distinction is the whole point: `Broken` means
/// we exercised the artifact and it's genuinely wrong (→ Unhealthy), while
/// `Unavailable` means we couldn't exercise it at all (→ Degraded). Reporting an
/// un-runnable probe as Unhealthy would page on our own misconfiguration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProbeFailure {
    /// The artifact is broken: a failing query, a compile error, an app task
    /// that errored, an agent that couldn't answer.
    Broken(String),
    /// The probe could not run: missing config, no semantic runner, an agent
    /// file that isn't there.
    Unavailable(String),
}
