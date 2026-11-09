//! Graceful-shutdown timings that more than one crate has to agree on.
//!
//! `serve` cancels the shutdown token, lets the HTTP listener finish in-flight
//! requests for at most [`GRACEFUL_DRAIN_DEADLINE`], then awaits the shutdown
//! hooks (bounded separately, in `router/recovery.rs`). Anything that flushes
//! this process's memory on shutdown — the custom-app usage drain — has to keep
//! going for the whole drain window, because the handlers still running in it
//! still produce work. It reads the deadline from here rather than keeping its
//! own copy of the number.

use std::time::Duration;

/// How long the serve command waits for in-flight requests after the shutdown
/// signal before it drops the serve future and moves on.
///
/// `axum::serve(...).with_graceful_shutdown(..)` otherwise waits for in-flight
/// requests *indefinitely*, so a single handler wedged on a 30s DB pool acquire
/// would delay the whole shutdown ~30s before the shutdown hooks even run.
/// Bounding it keeps total shutdown ≈ this deadline + the 10s hook budget,
/// comfortably inside a default 30s Kubernetes grace period. The TLS path uses
/// `axum_server`'s immediate `handle.shutdown()` and needs no equivalent.
pub const GRACEFUL_DRAIN_DEADLINE: Duration = Duration::from_secs(5);
