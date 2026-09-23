//! Integration tests for `agentic-airway`, in ONE binary.
//!
//! Cargo compiles every `tests/*.rs` into its own executable, each statically
//! linking the whole dependency graph — so a test *file* costs a link, not just
//! a compile. Grouping keeps that cost proportional to crates rather than to
//! files. Add a case as a `mod` below, not as a new `tests/*.rs`.
//!
//! See `internal-docs/testing.md` for the cost model.

mod cursor_reset_test;
mod harness;
mod pipeline_lease_test;
mod reset_test;
mod run_scoped_state_store_test;
mod sp_api_partner_type_test;
mod worker_integration;
mod workspace_state_test;
