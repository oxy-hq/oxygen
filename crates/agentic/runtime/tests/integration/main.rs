//! Integration tests for `agentic-runtime`, in ONE binary.
//!
//! Cargo compiles every `tests/*.rs` into its own executable, each statically
//! linking the whole dependency graph — so a test *file* costs a link, not just
//! a compile. Grouping keeps that cost proportional to crates rather than to
//! files. Add a case as a `mod` below, not as a new `tests/*.rs`.
//!
//! See `internal-docs/testing.md` for the cost model.

mod duplicate_delegation_test;
mod eviction_safety_test;
mod integration_tests;
mod reset_task_to_queued_test;
mod router_test;
mod run_feed_test;
mod stuck_run_sweeper_test;
mod suspension_clock_test;
mod tls_smoke_test;
mod unclaimed_clock_test;
